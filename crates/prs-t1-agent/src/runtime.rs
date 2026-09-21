use crate::framebuffer::{DisplayRegion, NativeDisplay, WaveformMode};
use crate::input::{EventReader, RawEvent};
use crate::refresh::{PageTone, RefreshPolicy, RefreshReason};
use crate::{display, input, reader, sync};
use embedded_graphics::geometry::Point;
use prs_markdown::reader::{ReaderError, ReaderEvent};
use std::fs::OpenOptions;
use std::future::Future;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::raw::c_int;
use std::path::Path;
use std::pin::Pin;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const EVENT_KEY: u16 = 1;
const EVENT_SYN: u16 = 0;
const EVENT_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const ABS_MT_TOUCH_MAJOR: u16 = 48;
const ABS_MT_POSITION_X: u16 = 53;
const ABS_MT_POSITION_Y: u16 = 54;
const ABS_MT_TRACKING_ID: u16 = 57;
const BTN_TOUCH: u16 = 330;
const KEY_POWER: u16 = 116;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
// The first key device (/dev/input/event0) reports the physical menu button
// as "Unknown" code 357 (the diagnostic label is E0 Unknown C357).
const KEY_MENU: u16 = 357;
const LONG_PRESS_MICROS: u64 = 2_000_000;
const MENU_HOLD_MICROS: u64 = 1_000_000;
const WAKE_LOCK_NAME: &str = "prs-t1-native-test";
const POWER_STATE_HELPER: &str = "/data/local/tmp/prs-t1-power-state";
const FRAMEWORK_STOP_TIMEOUT_SECONDS: u32 = 15;
const O_NONBLOCK: i32 = 0x800;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

unsafe extern "C" {
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    fn fork() -> c_int;
    fn getuid() -> u32;
    fn setsid() -> c_int;
    fn _exit(status: c_int) -> !;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuspendMode {
    EInk,
    Normal,
}

impl SuspendMode {
    pub fn parse(value: &str) -> io::Result<Self> {
        match value {
            "standby" => Ok(Self::EInk),
            "mem" => Ok(Self::Normal),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown suspend mode {value:?}; expected standby or mem"),
            )),
        }
    }

    fn state_bytes(self) -> &'static [u8] {
        match self {
            Self::EInk => b"standby\n",
            Self::Normal => b"mem\n",
        }
    }

    fn helper_arg(self) -> &'static str {
        match self {
            Self::EInk => "standby",
            Self::Normal => "mem",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::EInk => "EINK_STANDBY",
            Self::Normal => "NORMAL_MEM",
        }
    }
}

pub fn run(path: &Path, suspend_mode: SuspendMode) -> io::Result<()> {
    let mut display =
        NativeDisplay::open(path).map_err(|error| display_error("open native display", error))?;
    crate::status::ensure_native_ownership()?;
    let mut wake_lock = WakeLock::open().map_err(|error| display_error("open wake lock", error))?;
    wake_lock
        .acquire()
        .map_err(|error| display_error("acquire wake lock", error))?;
    let mut inputs =
        InputSet::open().map_err(|error| display_error("open input devices", error))?;
    let mut sync_task =
        SyncTask::new(path).map_err(|error| display_error("create sync runtime", error))?;
    let reader_config = reader::ReaderConfig::from_current_bundle().unwrap_or_else(|error| {
        eprintln!("standalone-test: no current PRSync bundle ({error}); using configured reader");
        reader::ReaderConfig::from_environment()
    });
    // The startup document can be a placeholder outside the PRSync library.
    // Keep reloads pinned to the same root used for atomic publication by the
    // long-lived synchronization task.
    let mut markdown_reader = reader::T1Reader::open_with_library_root(
        reader_config,
        reader::viewport_for_display(display.width(), display.height()),
        sync_task.library_root(),
    )
    .map_err(|error| display_error("open development Markdown reader", error))?;
    let mut state = UiState::new();
    if sync_task.start() {
        state.sync_started();
    }
    let mut refresh_policy = RefreshPolicy::default();
    let mut adb_restart_pending = false;

    eprintln!(
        "standalone-test: framebuffer={}x{}; zygote must already be stopped",
        display.width(),
        display.height()
    );
    redraw(
        &mut display,
        &state,
        &mut markdown_reader,
        wake_lock.is_held(),
        DirtyArea::Full,
        &mut refresh_policy,
        None,
    )
    .map_err(|error| display_error("initial redraw", error))?;

    loop {
        if adb_restart_pending && usb_power_online() {
            restart_adbd_if_enabled()?;
            adb_restart_pending = false;
        }
        let mut redraw_area = state.refresh_status_if_due().then_some(DirtyArea::Status);
        if let Some(dirty) = state.poll_menu_hold() {
            redraw_area = Some(match redraw_area {
                Some(existing) => existing.merge(dirty),
                None => dirty,
            });
        }
        let mut action = PowerAction::None;
        for source in &mut inputs.sources {
            loop {
                let Some(event) = source.reader.read_one()? else {
                    break;
                };
                let (dirty, event_action) = state.observe(source.kind, event);
                if let Some(dirty) = dirty {
                    redraw_area = Some(match redraw_area {
                        Some(existing) => existing.merge(dirty),
                        None => dirty,
                    });
                }
                if event_action != PowerAction::None {
                    action = event_action;
                }
                if let Some(operation) = state.take_reader_operation() {
                    let result = match operation {
                        ReaderOperation::PreviousPage => markdown_reader.previous_page(),
                        ReaderOperation::NextPage => markdown_reader.next_page(),
                        ReaderOperation::Back => markdown_reader.back(),
                        ReaderOperation::ReturnToEntryPoint => {
                            markdown_reader.return_to_entry_point()
                        }
                    };
                    let dirty = apply_reader_result(&mut state, &mut markdown_reader, result);
                    redraw_area = Some(match redraw_area {
                        Some(existing) => existing.merge(dirty),
                        None => dirty,
                    });
                }
                if let Some(point) = state.take_reader_tap() {
                    let result = markdown_reader.tap(point);
                    let dirty = apply_reader_result(&mut state, &mut markdown_reader, result);
                    redraw_area = Some(match redraw_area {
                        Some(existing) => existing.merge(dirty),
                        None => dirty,
                    });
                }
            }
        }

        if state.take_sync_request() {
            if sync_task.start() {
                state.sync_started();
                redraw_area = Some(DirtyArea::Full);
            }
        }
        if state.should_start_idle_sync(sync_task.idle_sync_interval()) && sync_task.start() {
            state.sync_started();
            redraw_area = Some(DirtyArea::Full);
        }
        while let Some(event) = sync_task.try_event() {
            state.apply_sync_event(event);
            redraw_area = Some(DirtyArea::Full);
        }
        if state.bundle_ready && state.is_idle() {
            match markdown_reader.reload_current_bundle() {
                Ok(()) => {
                    state.bundle_ready = false;
                    state.pending_handoff = None;
                    state.library_empty = markdown_reader.is_library_empty();
                    if let Err(error) = sync::cleanup_retired_generations(sync_task.library_root())
                    {
                        state.last_sync_failure = Some(format!("cleanup retired bundle: {error}"));
                        state.message = "Bundle ready; cleanup failed".into();
                        eprintln!("standalone-test: retired bundle cleanup failed: {error}");
                    } else {
                        state.message = if state.library_empty {
                            "Library cleared".into()
                        } else {
                            "New bundle ready".into()
                        };
                    }
                    redraw_area = Some(DirtyArea::Full);
                }
                Err(error) => {
                    state.last_sync_failure = Some(format!("reload current bundle: {error}"));
                    state.message = "Bundle handoff failed".into();
                    eprintln!("standalone-test: current bundle handoff failed: {error}");
                    redraw_area = Some(DirtyArea::Full);
                }
            }
        }

        match action {
            PowerAction::Sleep => sleep_cycle(
                &mut display,
                &mut wake_lock,
                &mut inputs,
                &mut state,
                &mut markdown_reader,
                suspend_mode,
                &mut refresh_policy,
                &mut sync_task,
            )
            .map(|()| {
                adb_restart_pending = true;
            })?,
            PowerAction::Reboot | PowerAction::PowerOff => {
                eprintln!("standalone-test: executing power action={action:?}");
                state.mode = "REBOOTING";
                state.message = match action {
                    PowerAction::Reboot => "REBOOT REQUESTED",
                    PowerAction::PowerOff => "POWER OFF REQUESTED",
                    PowerAction::None | PowerAction::Sleep => unreachable!(),
                }
                .into();
                redraw(
                    &mut display,
                    &state,
                    &mut markdown_reader,
                    wake_lock.is_held(),
                    DirtyArea::Full,
                    &mut refresh_policy,
                    Some(&mut sync_task),
                )
                .map_err(|error| display_error("reboot redraw", error))?;
                match action {
                    PowerAction::Reboot => request_reboot()?,
                    PowerAction::PowerOff => request_poweroff()?,
                    PowerAction::None | PowerAction::Sleep => unreachable!(),
                }
                return Ok(());
            }
            PowerAction::None if let Some(area) = redraw_area => {
                redraw(
                    &mut display,
                    &state,
                    &mut markdown_reader,
                    wake_lock.is_held(),
                    area,
                    &mut refresh_policy,
                    Some(&mut sync_task),
                )
                .map_err(|error| display_error("input redraw", error))?;
            }
            PowerAction::None => {}
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Start the native runtime from an Android-launched `su` process.
///
/// Android's `stop zygote` tears down the process group inherited by an APK
/// process. The caller must therefore create a new session before requesting
/// the stop; doing this in a shell script, even with `&`, is not sufficient.
pub fn launch(path: &Path, suspend_mode: SuspendMode) -> io::Result<()> {
    if unsafe { getuid() } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "launch-standalone requires a root su session",
        ));
    }

    let child = unsafe { fork() };
    if child < 0 {
        return Err(io::Error::last_os_error());
    }
    if child > 0 {
        // The APK-launched side must return to Superuser promptly. The
        // detached child owns the rest of the handoff.
        unsafe { _exit(0) };
    }

    if unsafe { setsid() } < 0 {
        return Err(io::Error::last_os_error());
    }

    eprintln!("launch-standalone: detached session; stopping zygote");
    let status = Command::new("/system/bin/stop").arg("zygote").status()?;
    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("stop zygote exited with {status}"),
        ));
    }
    wait_for_framework_stop()?;
    eprintln!("launch-standalone: Android framework stopped; entering native test");
    run(path, suspend_mode)
}

fn wait_for_framework_stop() -> io::Result<()> {
    for attempt in 0..=FRAMEWORK_STOP_TIMEOUT_SECONDS {
        match crate::status::ensure_native_ownership() {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::Other
                ) && attempt < FRAMEWORK_STOP_TIMEOUT_SECONDS =>
            {
                thread::sleep(Duration::from_secs(1));
            }
            Err(error) if attempt >= FRAMEWORK_STOP_TIMEOUT_SECONDS => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "Android framework did not stop within {} seconds: {error}",
                        FRAMEWORK_STOP_TIMEOUT_SECONDS
                    ),
                ));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("framework stop loop always returns");
}

fn redraw(
    display: &mut NativeDisplay,
    state: &UiState,
    markdown_reader: &mut reader::T1Reader,
    wake_lock_held: bool,
    area: DirtyArea,
    refresh_policy: &mut RefreshPolicy,
    sync_task: Option<&mut SyncTask>,
) -> io::Result<()> {
    let lines = screen_lines(state, wake_lock_held);
    let reason = area.reason(state.page);
    let plan = refresh_policy.plan(reason);
    eprintln!(
        "standalone-test: refresh policy reason={} waveform={} completion={} force={} cleanup={}",
        reason.label(),
        plan.waveform().label(),
        if plan.wait_for_completion() {
            "wait"
        } else {
            "nowait"
        },
        plan.force_refresh(),
        plan.is_cleanup(),
    );
    if state.page == UiPage::DisplayTest {
        let result = display::draw_interactive_display_test(
            display,
            plan.waveform(),
            plan.wait_for_completion(),
            plan.force_refresh(),
        );
        if result.is_ok() {
            refresh_policy.record_success(reason, plan);
        }
        return finish_redraw(result, sync_task);
    }
    if state.sync_active {
        if let Some(approval_url) = state.approval_url.as_deref() {
            return finish_redraw(
                display::draw_authorization_qr(display, approval_url),
                sync_task,
            );
        }
    }
    if state.page == UiPage::Home {
        if state.library_empty {
            let result = display::draw_screen(
                display,
                &lines,
                area.region(display, state.page),
                plan.waveform(),
                plan.wait_for_completion(),
                plan.force_refresh(),
            );
            if result.is_ok() {
                refresh_policy.record_success(reason, plan);
            }
            return finish_redraw(result, sync_task);
        }
        let status_line = lines.first().map(String::as_str).unwrap_or_default();
        let result = markdown_reader.draw(
            display,
            status_line,
            &state.message,
            area.region(display, state.page),
            plan,
        );
        if result.is_ok() {
            refresh_policy.record_success(reason, plan);
        }
        return finish_redraw(result, sync_task);
    }
    let result = display::draw_screen(
        display,
        &lines,
        area.region(display, state.page),
        plan.waveform(),
        plan.wait_for_completion(),
        plan.force_refresh(),
    );
    if result.is_ok() {
        refresh_policy.record_success(reason, plan);
    }
    finish_redraw(result, sync_task)
}

fn finish_redraw(result: io::Result<()>, sync_task: Option<&mut SyncTask>) -> io::Result<()> {
    let Err(render_error) = result else {
        return Ok(());
    };
    let Some(sync_task) = sync_task else {
        return Err(render_error);
    };
    match sync_task.cancel() {
        Ok(()) => Err(render_error),
        Err(cleanup_error) => Err(io::Error::new(
            render_error.kind(),
            format!("display render failed: {render_error}; Wi-Fi cleanup failed: {cleanup_error}"),
        )),
    }
}

fn reader_event_dirty_area(event: &ReaderEvent, tone: PageTone) -> Option<DirtyArea> {
    match event {
        ReaderEvent::PageChanged { .. }
        | ReaderEvent::Navigated { .. }
        | ReaderEvent::Back { .. }
        | ReaderEvent::Forward { .. } => Some(DirtyArea::PageTurn(tone)),
        ReaderEvent::ExternalUrl(_) | ReaderEvent::Asset(_) | ReaderEvent::NoAction => {
            Some(DirtyArea::Interaction)
        }
        ReaderEvent::Opened { .. } => None,
    }
}

fn reader_event_message(event: &ReaderEvent) -> String {
    match event {
        ReaderEvent::Opened { page_count, .. } => format!("Opened document ({page_count} pages)"),
        ReaderEvent::PageChanged { page, page_count } => {
            format!("Page {} of {page_count}", page.saturating_add(1))
        }
        ReaderEvent::Navigated {
            location,
            page,
            page_count,
        } => format!(
            "Opened {} page {} of {page_count}",
            location.document.as_ref(),
            page.saturating_add(1)
        ),
        ReaderEvent::Back {
            location,
            page,
            page_count,
        } => format!(
            "Back to {} page {} of {page_count}",
            location.document.as_ref(),
            page.saturating_add(1)
        ),
        ReaderEvent::Forward {
            location,
            page,
            page_count,
        } => format!(
            "Forward to {} page {} of {page_count}",
            location.document.as_ref(),
            page.saturating_add(1)
        ),
        ReaderEvent::ExternalUrl(url) => format!("External link: {url}"),
        ReaderEvent::Asset(path) => format!("Asset selected: {}", path.display()),
        ReaderEvent::NoAction => "Reading unchanged".into(),
    }
}

fn apply_reader_result(
    state: &mut UiState,
    markdown_reader: &mut reader::T1Reader,
    result: Result<ReaderEvent, ReaderError>,
) -> DirtyArea {
    match result {
        Ok(event) => {
            state.message = reader_event_message(&event);
            let dirty = reader_event_dirty_area(&event, markdown_reader.current_page_tone())
                .unwrap_or(DirtyArea::Full);
            match event {
                ReaderEvent::ExternalUrl(url) => {
                    markdown_reader.set_external_url_notice(Some(url));
                }
                ReaderEvent::NoAction => {}
                ReaderEvent::Opened { .. }
                | ReaderEvent::PageChanged { .. }
                | ReaderEvent::Navigated { .. }
                | ReaderEvent::Back { .. }
                | ReaderEvent::Forward { .. }
                | ReaderEvent::Asset(_) => markdown_reader.set_external_url_notice(None),
            }
            dirty
        }
        Err(error) => {
            state.message = format!("Reader error: {error}");
            markdown_reader.set_external_url_notice(None);
            eprintln!("standalone-test: Markdown operation failed: {error}");
            DirtyArea::Full
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirtyArea {
    Full,
    Status,
    PageTurn(PageTone),
    Interaction,
    Touch,
    Key,
    Power,
}

impl DirtyArea {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::PageTurn(left), Self::PageTurn(right)) => Self::PageTurn(match (left, right) {
                (PageTone::Grayscale, _) | (_, PageTone::Grayscale) => PageTone::Grayscale,
                (PageTone::Monochrome, PageTone::Monochrome) => PageTone::Monochrome,
            }),
            (Self::PageTurn(_), Self::Status) | (Self::Status, Self::PageTurn(_)) => Self::Full,
            (Self::PageTurn(tone), Self::Interaction)
            | (Self::Interaction, Self::PageTurn(tone)) => Self::PageTurn(tone),
            (Self::Key, Self::Power) | (Self::Power, Self::Key) => Self::Power,
            (left, right) if left == right => left,
            _ => Self::Full,
        }
    }

    fn reason(self, page: UiPage) -> RefreshReason {
        match self {
            Self::Full => RefreshReason::FullRedraw,
            Self::PageTurn(tone) => RefreshReason::PageTurn(tone),
            Self::Status if page == UiPage::Home => RefreshReason::StatusBar,
            Self::Status | Self::Interaction | Self::Touch | Self::Key | Self::Power => {
                RefreshReason::Transient
            }
        }
    }

    fn region(self, display: &NativeDisplay, page: UiPage) -> DisplayRegion {
        let content_region = || {
            DisplayRegion::new(
                0,
                display::STATUS_BAR_HEIGHT as u32,
                display.width(),
                display
                    .height()
                    .saturating_sub(display::STATUS_BAR_HEIGHT as u32),
            )
        };
        let region = match self {
            Self::Full => DisplayRegion::full(display.width(), display.height()),
            Self::PageTurn(_) | Self::Interaction => content_region(),
            Self::Status if page == UiPage::Home => {
                DisplayRegion::new(0, 0, display.width(), display::STATUS_BAR_HEIGHT as u32)
            }
            Self::Status => DisplayRegion::new(0, 0, display.width(), 640),
            // The details page keeps the touch diagnostics near the bottom
            // of the content area. Keep the update well inside the display.
            Self::Touch => DisplayRegion::new(20, 460, 560, 130),
            // Key and power diagnostics share one region so a power press can
            // update the key row, power row, and status message together.
            Self::Key | Self::Power => DisplayRegion::new(20, 535, 560, 105),
        };
        region.bounded(display.width(), display.height())
    }
}

fn screen_lines(state: &UiState, wake_lock_held: bool) -> Vec<String> {
    let status = &state.status;
    let touch = state
        .last_touch
        .map(|event| {
            format!(
                "Touch: {} C{} V{}",
                pretty_event_name(input::event_code_name(event.event_type, event.code)),
                event.code,
                event.value
            )
        })
        .unwrap_or_else(|| "Touch: none".into());
    let coordinates = if state.touch_seen {
        format!("Touch: X {}  Y {}", state.touch_x, state.touch_y)
    } else {
        "Touch: X ---  Y ---".into()
    };
    let key = state
        .last_key
        .map(|(source, event)| {
            format!(
                "Key: {} {} C{} V{}",
                source.label(),
                pretty_event_name(input::event_code_name(event.event_type, event.code)),
                event.code,
                event.value
            )
        })
        .unwrap_or_else(|| "Key: none".into());
    let battery_level = number_or_unknown(status.battery.capacity_percent);
    let battery_state = uppercase_or_unknown(status.battery.status.as_deref());
    let temperature = number_or_unknown(status.battery.temperature);
    let ac = bool_label(status.power.ac_online);
    let usb_power = bool_label(status.power.usb_online);
    let usb_connected = bool_label(status.usb.physical_connected);
    let wifi_state = if status.wifi.interface_present {
        uppercase_or_unknown(status.wifi.operstate.as_deref())
    } else {
        "OFF".into()
    };
    let supplicant = uppercase_or_unknown(status.wifi.supplicant_state.as_deref());
    let adb = if status.adb.process_running {
        "ON"
    } else {
        "OFF"
    };
    let framebuffer = match status.screen.framebuffer_state {
        Some(0) => "ACTIVE",
        Some(_) => "OTHER",
        None => "UNKNOWN",
    };
    let zygote = if status.android.zygote_running {
        "RUN"
    } else {
        "STOP"
    };
    let dispd = if status.android.dispd_running {
        "RUN"
    } else {
        "STOP"
    };
    let battery_label = percent_label(&battery_level);
    let mode_label = if state.mode == "ACTIVE" {
        String::new()
    } else {
        pretty_value(state.mode)
    };
    let header = format!(
        "{}|{}|{}|{}|{}|{}",
        battery_label,
        pretty_value(&wifi_state),
        pretty_value(usb_connected),
        pretty_value(adb),
        mode_label,
        short_clock()
    );

    if state.page == UiPage::Home {
        return vec![
            header,
            "PRS-T1 Native Shell".into(),
            "Tap status for details".into(),
        ];
    }
    if state.page == UiPage::DisplayTest {
        return vec![header, "Display Test".into(), state.message.clone()];
    }

    let mut lines = vec![header, "Details / Settings".into()];
    lines.extend([
        "Power".into(),
        format!(
            "Battery {}  {}",
            percent_label(&battery_level),
            pretty_value(&battery_state)
        ),
        format!(
            "Health {}  Voltage {}",
            pretty_value(&uppercase_or_unknown(status.battery.health.as_deref())),
            voltage_label(status.battery.voltage_uv)
        ),
        format!(
            "Temperature {} C  AC {}  USB {}",
            temperature,
            pretty_value(ac),
            pretty_value(usb_power)
        ),
        "Connectivity".into(),
        format!(
            "WiFi {} {}",
            status.wifi.interface.to_ascii_lowercase(),
            pretty_value(&wifi_state)
        ),
        format!("Supplicant {}", pretty_value(&supplicant)),
        format!(
            "USB {}  Gadget {}",
            pretty_value(usb_connected),
            pretty_value(&uppercase_or_unknown(status.usb.gadget_state.as_deref()))
        ),
        format!(
            "USB functions {}",
            pretty_value(&uppercase_or_unknown(
                status.usb.gadget_functions.as_deref()
            ))
        ),
        "Synchronization".into(),
        format!("Sync {}", if state.sync_active { "active" } else { "idle" }),
        format!(
            "Failure {}",
            state.last_sync_failure.as_deref().unwrap_or("none")
        ),
        format!(
            "ADB process {}  Service {}",
            pretty_value(adb),
            pretty_value(&uppercase_or_unknown(status.adb.service_state.as_deref()))
        ),
        "System".into(),
        format!(
            "Framebuffer {}  Rotate {}",
            pretty_value(framebuffer),
            number_or_unknown(status.screen.rotate)
        ),
        format!(
            "Android: zygote {}  dispd {}",
            pretty_value(zygote),
            pretty_value(dispd)
        ),
        format!(
            "Wake lock {}",
            pretty_value(if wake_lock_held { "yes" } else { "no" })
        ),
        format!("Date {}", date_time()),
        "Storage".into(),
        format!(
            "Data {} KiB free",
            number_or_unknown(status.storage.data.available_kib)
        ),
        format!(
            "SD card {} KiB free",
            number_or_unknown(status.storage.sdcard.available_kib)
        ),
        "Input".into(),
        coordinates,
        touch,
        format!("Touch events {}", state.touch_events),
        key,
        format!("Key events {}", state.key_events),
        format!(
            "Power last {}",
            state
                .last_power_duration_ms
                .map(|duration| format!("{}ms", duration))
                .unwrap_or_else(|| "none".into())
        ),
    ]);
    lines
}

fn short_clock() -> String {
    Command::new("/system/bin/date")
        .arg("+%H:%M")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "--:--".into())
}

fn date_time() -> String {
    Command::new("/system/bin/date")
        .arg("+%d %b %Y %H:%M")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Unknown".into())
}

fn percent_label(value: &str) -> String {
    if value == "UNKNOWN" {
        "Unknown".into()
    } else {
        format!("{value}%")
    }
}

fn voltage_label(value: Option<i64>) -> String {
    value
        .map(|microvolts| {
            let whole = microvolts / 1_000_000;
            let fractional = (microvolts.abs() % 1_000_000) / 10_000;
            format!("{whole}.{fractional:02} V")
        })
        .unwrap_or_else(|| "Unknown".into())
}

fn display_error(stage: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{stage}: {error}"))
}

fn sleep_cycle(
    display: &mut NativeDisplay,
    wake_lock: &mut WakeLock,
    inputs: &mut InputSet,
    state: &mut UiState,
    markdown_reader: &mut reader::T1Reader,
    suspend_mode: SuspendMode,
    refresh_policy: &mut RefreshPolicy,
    sync_task: &mut SyncTask,
) -> io::Result<()> {
    state.mode = "SLEEPING";
    state.message = match suspend_mode {
        SuspendMode::EInk => "E-ink standby mode",
        SuspendMode::Normal => "Normal mem mode",
    }
    .into();
    eprintln!("standalone-test: drawing pre-suspend screen");
    redraw(
        display,
        state,
        markdown_reader,
        wake_lock.is_held(),
        DirtyArea::Full,
        refresh_policy,
        Some(&mut *sync_task),
    )
    .map_err(|error| display_error("pre-suspend redraw", error))?;
    let standby_lines = screen_lines(state, wake_lock.is_held());
    let standby = match state.page {
        UiPage::Home => markdown_reader.render_frame(
            standby_lines
                .first()
                .map(String::as_str)
                .unwrap_or_default(),
            &state.message,
            display.width(),
            display.height(),
        )?,
        UiPage::DisplayTest => display::render_interactive_display_test(
            display.width() as usize,
            display.height() as usize,
        ),
        UiPage::Details => display::standby_screen(
            &standby_lines,
            display.width() as usize,
            display.height() as usize,
        ),
    };
    display
        .write_standby(&standby)
        .map_err(|error| display_error("standby screen write", error))?;
    eprintln!("standalone-test: standby screen supplied");
    display.prepare_for_suspend();

    eprintln!(
        "standalone-test: requesting suspend mode={}",
        suspend_mode.label()
    );
    let suspend_started = Instant::now();
    let suspend_result = wake_lock
        .release()
        .and_then(|_| request_suspend(suspend_mode));
    eprintln!("standalone-test: suspend request queued: {suspend_result:?}");
    let sleep_result = suspend_result.and_then(|_| wait_for_display_wake(inputs, state));
    let suspend_elapsed_ms = suspend_started.elapsed().as_millis() as u64;
    eprintln!(
        "standalone-test: suspend/wake wait returned: {sleep_result:?} elapsed_ms={suspend_elapsed_ms}"
    );
    let acquire_result = wake_lock.acquire();
    eprintln!("standalone-test: wake lock reacquire returned: {acquire_result:?}");
    let resume_result = display.resume_after_suspend();
    eprintln!("standalone-test: framebuffer remap returned: {resume_result:?}");

    sleep_result?;
    acquire_result?;
    resume_result.map_err(|error| display_error("post-resume framebuffer remap", error))?;

    state.mode = "ACTIVE";
    state.refresh_status();
    state.message = format!("Woke after {suspend_elapsed_ms}ms");
    state.last_power_duration_ms = None;
    state.ignore_power_until = Some(Instant::now() + Duration::from_secs(2));
    redraw(
        display,
        state,
        markdown_reader,
        wake_lock.is_held(),
        DirtyArea::Full,
        refresh_policy,
        Some(&mut *sync_task),
    )
    .map_err(|error| display_error("post-resume redraw", error))
}

fn request_suspend(mode: SuspendMode) -> io::Result<()> {
    if Path::new(POWER_STATE_HELPER).exists() {
        eprintln!(
            "standalone-test: requesting vendor suspend helper state={}",
            mode.helper_arg()
        );
        return run_power_state_helper(mode.helper_arg());
    }

    eprintln!(
        "standalone-test: vendor suspend helper unavailable; using raw sysfs state={} fallback",
        mode.label()
    );
    let mut state = OpenOptions::new().write(true).open("/sys/power/state")?;
    state.write_all(mode.state_bytes())?;
    state.flush()
}

fn wait_for_display_wake(inputs: &mut InputSet, state: &mut UiState) -> io::Result<()> {
    let mut wake = DisplayWakeReader::spawn()?;
    let mut buffer = [0u8; 16];
    let mut resume_requested = false;

    loop {
        match wake.output.read(&mut buffer) {
            Ok(bytes) if bytes > 0 => {
                eprintln!("standalone-test: display wake barrier released bytes={bytes}");
                return Ok(());
            }
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "display wake barrier reader exited without a result",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }

        if let Some(status) = wake.child.try_wait()? {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("display wake barrier reader exited with {status}"),
            ));
        }

        for source in &mut inputs.sources {
            loop {
                let Some(event) = source.reader.read_one()? else {
                    break;
                };
                if source.kind.is_power()
                    && event.event_type == EVENT_KEY
                    && event.code == KEY_POWER
                {
                    state.last_key = Some((source.kind, event));
                    state.key_events += 1;
                    eprintln!(
                        "standalone-test: wake-side power event source={} value={} timestamp_us={}",
                        source.kind.label(),
                        event.value,
                        event.timestamp_micros(),
                    );
                    if !resume_requested && matches!(event.value, 0..=2) {
                        eprintln!("standalone-test: requesting early resume");
                        request_resume()?;
                        eprintln!("standalone-test: deferring adbd restart until USB reconnect");
                        resume_requested = true;
                    }
                }
            }
        }

        thread::sleep(Duration::from_millis(20));
    }
}

struct DisplayWakeReader {
    child: Child,
    output: ChildStdout,
}

impl DisplayWakeReader {
    fn spawn() -> io::Result<Self> {
        let mut child = Command::new("/system/bin/cat")
            .arg("/sys/power/wait_for_fb_wake")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let output = match child.stdout.take() {
            Some(output) => output,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "display wake barrier reader has no stdout pipe",
                ));
            }
        };
        if let Err(error) = set_nonblocking(output.as_raw_fd()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(Self { child, output })
    }
}

impl Drop for DisplayWakeReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn set_nonblocking(fd: c_int) -> io::Result<()> {
    let flags = unsafe { fcntl(fd, F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { fcntl(fd, F_SETFL, flags | O_NONBLOCK) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn request_resume() -> io::Result<()> {
    if Path::new(POWER_STATE_HELPER).exists() {
        eprintln!("standalone-test: requesting vendor resume helper state=on");
        return run_power_state_helper("on");
    }

    eprintln!("standalone-test: vendor resume helper unavailable; using raw sysfs on fallback");
    let mut state = OpenOptions::new().write(true).open("/sys/power/state")?;
    state.write_all(b"on\n")?;
    state.flush()
}

fn restart_adbd_if_enabled() -> io::Result<()> {
    let enabled = Command::new("/system/bin/getprop")
        .arg("persist.service.adb.enable")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "1")
        .unwrap_or(false);
    if !enabled {
        eprintln!("standalone-test: ADB restart skipped; persistent ADB is disabled");
        return Ok(());
    }

    eprintln!("standalone-test: restarting adbd after wake");
    let stop = Command::new("/system/bin/stop").arg("adbd").status()?;
    if !stop.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("stop adbd exited with {stop}"),
        ));
    }
    thread::sleep(Duration::from_millis(200));
    let start = Command::new("/system/bin/start").arg("adbd").status()?;
    if !start.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("start adbd exited with {start}"),
        ));
    }
    eprintln!("standalone-test: adbd restart requested");
    Ok(())
}

fn usb_power_online() -> bool {
    std::fs::read_to_string("/sys/class/power_supply/sub_cpu_usb/online")
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

fn uppercase_or_unknown(value: Option<&str>) -> String {
    value
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_else(|| "UNKNOWN".into())
}

fn bool_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "ON",
        Some(false) => "OFF",
        None => "UNKNOWN",
    }
}

fn number_or_unknown(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "UNKNOWN".into())
}

fn pretty_value(value: &str) -> String {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    std::iter::once(first.to_ascii_uppercase())
        .chain(characters.flat_map(|character| character.to_lowercase()))
        .collect()
}

fn pretty_event_name(value: &str) -> String {
    value
        .split('_')
        .map(pretty_value)
        .collect::<Vec<_>>()
        .join(" ")
}

fn run_power_state_helper(state: &str) -> io::Result<()> {
    let status = Command::new(POWER_STATE_HELPER).arg(state).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("power-state helper exited with {status}"),
        ))
    }
}

fn request_reboot() -> io::Result<()> {
    eprintln!("standalone-test: invoking /system/bin/reboot reboot");
    let status = Command::new("/system/bin/reboot").arg("reboot").status()?;
    eprintln!("standalone-test: /system/bin/reboot reboot returned {status}");
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("reboot command exited with {status}"),
        ))
    }
}

fn request_poweroff() -> io::Result<()> {
    eprintln!("standalone-test: invoking /system/bin/reboot -p");
    let status = Command::new("/system/bin/reboot").arg("-p").status()?;
    eprintln!("standalone-test: /system/bin/reboot -p returned {status}");
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("power-off command exited with {status}"),
        ))
    }
}

struct WakeLock {
    lock: std::fs::File,
    unlock: std::fs::File,
    held: bool,
}

impl WakeLock {
    fn open() -> io::Result<Self> {
        Ok(Self {
            lock: OpenOptions::new()
                .write(true)
                .open("/sys/power/wake_lock")?,
            unlock: OpenOptions::new()
                .write(true)
                .open("/sys/power/wake_unlock")?,
            held: false,
        })
    }

    fn acquire(&mut self) -> io::Result<()> {
        if self.held {
            return Ok(());
        }
        self.lock.write_all(WAKE_LOCK_NAME.as_bytes())?;
        self.lock.flush()?;
        self.held = true;
        Ok(())
    }

    fn release(&mut self) -> io::Result<()> {
        if !self.held {
            return Ok(());
        }
        self.unlock.write_all(WAKE_LOCK_NAME.as_bytes())?;
        self.unlock.flush()?;
        self.held = false;
        Ok(())
    }

    fn is_held(&self) -> bool {
        self.held
    }
}

impl Drop for WakeLock {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputSourceKind {
    Keys,
    Touch,
    PmicPower,
    SubCpuPower,
}

impl InputSourceKind {
    fn label(self) -> &'static str {
        match self {
            Self::Keys => "E0",
            Self::Touch => "E1",
            Self::PmicPower => "E2",
            Self::SubCpuPower => "E4",
        }
    }

    fn is_power(self) -> bool {
        matches!(self, Self::PmicPower | Self::SubCpuPower)
    }
}

struct InputSource {
    kind: InputSourceKind,
    reader: EventReader,
}

struct InputSet {
    sources: Vec<InputSource>,
}

impl InputSet {
    fn open() -> io::Result<Self> {
        let devices = [
            (InputSourceKind::Keys, "/dev/input/event0"),
            (InputSourceKind::Touch, "/dev/input/event1"),
            (InputSourceKind::PmicPower, "/dev/input/event2"),
            (InputSourceKind::SubCpuPower, "/dev/input/event4"),
        ];
        let sources = devices
            .into_iter()
            .map(|(kind, path)| {
                Ok(InputSource {
                    kind,
                    reader: EventReader::open(Path::new(path))?,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Self { sources })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PowerAction {
    None,
    Sleep,
    Reboot,
    PowerOff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReaderOperation {
    PreviousPage,
    NextPage,
    Back,
    ReturnToEntryPoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BundleHandoff {
    Updated,
    Cleared,
}

enum SyncEvent {
    ApprovalUrl(String),
    Finished(Result<sync::SyncOutcome, String>),
}

type SyncFuture = Pin<Box<dyn Future<Output = io::Result<sync::SyncOutcome>>>>;

struct SyncTask {
    config: sync::SyncConfig,
    client: sync::SyncClientHandle,
    runtime: tokio::runtime::Runtime,
    progress: sync::SyncProgress,
    future: Option<SyncFuture>,
    completion: Option<Result<sync::SyncOutcome, String>>,
    busy: bool,
}

impl SyncTask {
    fn new(framebuffer: &Path) -> io::Result<Self> {
        let config = sync::SyncConfig::for_runtime(framebuffer).map_err(io::Error::other)?;
        let progress = sync::new_progress();
        let client = sync::new_client(config.clone(), progress.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| io::Error::other(format!("could not create sync runtime: {error}")))?;
        Ok(Self {
            config,
            client,
            runtime,
            progress,
            future: None,
            completion: None,
            busy: false,
        })
    }

    fn start(&mut self) -> bool {
        if self.busy {
            return false;
        }
        self.progress.borrow_mut().clear();
        self.completion = None;
        // Keep the client across attempts so the read-only authorization
        // session and conditional manifest snapshot remain boot-scoped.
        self.future = Some(Box::pin(crate::wifi::run_sync_client_outcome_async(
            self.client.clone(),
        )));
        self.busy = true;
        true
    }

    fn try_event(&mut self) -> Option<SyncEvent> {
        if let Some(event) = sync::take_progress(&self.progress) {
            return match event {
                sync::SyncProgressEvent::ApprovalUrl(url) => Some(SyncEvent::ApprovalUrl(url)),
            };
        }
        if let Some(result) = self.completion.take() {
            self.busy = false;
            return Some(SyncEvent::Finished(result));
        }
        if let Some(future) = self.future.as_mut() {
            if let Some(result) = self.runtime.block_on(poll_once(future.as_mut())) {
                self.future = None;
                self.completion = Some(result.map_err(|error| error.to_string()));
            }
        }
        if let Some(event) = sync::take_progress(&self.progress) {
            return match event {
                sync::SyncProgressEvent::ApprovalUrl(url) => Some(SyncEvent::ApprovalUrl(url)),
            };
        }
        self.completion.take().map(|result| {
            self.busy = false;
            SyncEvent::Finished(result)
        })
    }

    fn library_root(&self) -> &Path {
        self.config.library_root()
    }

    fn idle_sync_interval(&self) -> Duration {
        self.config.idle_sync_interval()
    }

    fn cancel(&mut self) -> io::Result<()> {
        let active = self.future.take().is_some();
        self.completion = None;
        self.busy = false;
        if active {
            self.runtime
                .block_on(crate::wifi::shutdown_after_sync_cancellation_async())
        } else {
            Ok(())
        }
    }
}

async fn poll_once<F>(future: Pin<&mut F>) -> Option<F::Output>
where
    F: Future + ?Sized,
{
    tokio::select! {
        result = future => Some(result),
        _ = tokio::task::yield_now() => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiPage {
    Home,
    Details,
    DisplayTest,
}

struct UiState {
    page: UiPage,
    mode: &'static str,
    message: String,
    reader_tap: Option<Point>,
    reader_operation: Option<ReaderOperation>,
    touch_seen: bool,
    touch_x: i32,
    touch_y: i32,
    touch_down: bool,
    touch_release_pending: bool,
    last_touch: Option<RawEvent>,
    last_key: Option<(InputSourceKind, RawEvent)>,
    touch_events: u64,
    key_events: u64,
    power_press_us: Option<u64>,
    menu_press_us: Option<u64>,
    menu_pressed_at: Option<Instant>,
    menu_hold_triggered: bool,
    last_power_duration_ms: Option<u64>,
    ignore_power_until: Option<Instant>,
    status: crate::status::StatusSnapshot,
    last_status_refresh: Instant,
    sync_active: bool,
    sync_requested: bool,
    approval_url: Option<String>,
    pub(crate) bundle_ready: bool,
    pending_handoff: Option<BundleHandoff>,
    library_empty: bool,
    last_sync_failure: Option<String>,
    last_activity: Instant,
    last_sync_completion: Instant,
}

impl UiState {
    fn new() -> Self {
        Self {
            page: UiPage::Home,
            mode: "ACTIVE",
            message: "Input ready".into(),
            reader_tap: None,
            reader_operation: None,
            touch_seen: false,
            touch_x: 0,
            touch_y: 0,
            touch_down: false,
            touch_release_pending: false,
            last_touch: None,
            last_key: None,
            touch_events: 0,
            key_events: 0,
            power_press_us: None,
            menu_press_us: None,
            menu_pressed_at: None,
            menu_hold_triggered: false,
            last_power_duration_ms: None,
            ignore_power_until: None,
            status: crate::status::collect(),
            last_status_refresh: Instant::now(),
            sync_active: false,
            sync_requested: false,
            approval_url: None,
            bundle_ready: false,
            pending_handoff: None,
            library_empty: false,
            last_sync_failure: None,
            last_activity: Instant::now(),
            last_sync_completion: Instant::now(),
        }
    }

    fn refresh_status_if_due(&mut self) -> bool {
        if self.last_status_refresh.elapsed() < STATUS_REFRESH_INTERVAL {
            return false;
        }
        self.refresh_status();
        true
    }

    fn refresh_status(&mut self) {
        self.status = crate::status::collect();
        self.last_status_refresh = Instant::now();
    }

    fn take_reader_tap(&mut self) -> Option<Point> {
        self.reader_tap.take()
    }

    fn take_reader_operation(&mut self) -> Option<ReaderOperation> {
        self.reader_operation.take()
    }

    fn take_sync_request(&mut self) -> bool {
        std::mem::take(&mut self.sync_requested)
    }

    fn sync_started(&mut self) {
        self.sync_active = true;
        self.approval_url = None;
        self.message = "Synchronizing…".into();
    }

    fn apply_sync_event(&mut self, event: SyncEvent) {
        match event {
            SyncEvent::ApprovalUrl(url) => {
                self.approval_url = Some(url);
                self.message = "Scan to authorize synchronization".into();
            }
            SyncEvent::Finished(Ok(outcome)) => {
                self.sync_active = false;
                self.approval_url = None;
                self.last_sync_completion = Instant::now();
                self.last_sync_failure = None;
                match outcome {
                    sync::SyncOutcome::Updated { .. } => {
                        self.bundle_ready = true;
                        self.pending_handoff = Some(BundleHandoff::Updated);
                        self.message = "Synchronization complete".into();
                    }
                    sync::SyncOutcome::Cleared { .. } => {
                        self.bundle_ready = true;
                        self.pending_handoff = Some(BundleHandoff::Cleared);
                        self.message = "Library cleared".into();
                    }
                    sync::SyncOutcome::Unchanged { .. } => {
                        self.message = "Already current".into();
                    }
                }
            }
            SyncEvent::Finished(Err(error)) => {
                self.sync_active = false;
                self.approval_url = None;
                self.last_sync_completion = Instant::now();
                self.last_sync_failure = Some(error.clone());
                self.message = "Synchronization failed".into();
                eprintln!("standalone-test: synchronization failed: {error}");
            }
        }
    }

    fn should_start_idle_sync(&self, idle_sync_interval: Duration) -> bool {
        !self.sync_active
            && !self.bundle_ready
            && matches!(self.page, UiPage::Home | UiPage::Details)
            && self.last_sync_completion.elapsed() >= idle_sync_interval
            && self.last_activity.elapsed() >= idle_sync_interval
    }

    fn is_idle(&self) -> bool {
        self.last_activity.elapsed() >= Duration::from_secs(2)
    }

    fn observe(
        &mut self,
        source: InputSourceKind,
        event: RawEvent,
    ) -> (Option<DirtyArea>, PowerAction) {
        self.last_activity = Instant::now();
        if source == InputSourceKind::Touch {
            self.last_touch = Some(event);
            if event.event_type == EVENT_ABS {
                match event.code {
                    ABS_MT_POSITION_X => {
                        self.touch_x = event.value;
                        self.touch_seen = true;
                    }
                    ABS_MT_POSITION_Y => {
                        self.touch_y = event.value;
                        self.touch_seen = true;
                    }
                    // The T1's legacy compatibility axes are physically
                    // oriented 800x600 while the framebuffer is 600x800.
                    // Normalize them to screen x/y before hit testing.
                    ABS_X => {
                        self.touch_y = event.value;
                        self.touch_seen = true;
                    }
                    ABS_Y => {
                        self.touch_x = event.value;
                        self.touch_seen = true;
                    }
                    _ => {}
                }
            }
            if event.event_type == EVENT_KEY && event.code == BTN_TOUCH {
                if event.value != 0 {
                    self.touch_down = true;
                } else if self.touch_down {
                    self.touch_down = false;
                    let action = self.activate_tap();
                    eprintln!(
                        "standalone-test: touch tap x={} y={} page={:?} action={action:?}",
                        self.touch_x, self.touch_y, self.page
                    );
                    return (Some(self.touch_release_dirty()), action);
                }
            }
            if event.event_type == EVENT_ABS && event.code == ABS_MT_TOUCH_MAJOR {
                if event.value > 0 {
                    self.touch_down = true;
                    self.touch_release_pending = false;
                } else if self.touch_down {
                    self.touch_release_pending = true;
                }
            }
            if event.event_type == EVENT_ABS && event.code == ABS_MT_TRACKING_ID {
                if event.value >= 0 {
                    self.touch_down = true;
                    self.touch_release_pending = false;
                } else if self.touch_down {
                    self.touch_release_pending = true;
                }
            }
            if event.event_type == EVENT_SYN && event.code == SYN_REPORT {
                self.touch_events += 1;
                if self.touch_release_pending {
                    self.touch_down = false;
                    self.touch_release_pending = false;
                    let action = self.activate_tap();
                    eprintln!(
                        "standalone-test: touch tap x={} y={} page={:?} action={action:?}",
                        self.touch_x, self.touch_y, self.page
                    );
                    return (Some(self.touch_release_dirty()), action);
                }
                return (Some(DirtyArea::Touch), PowerAction::None);
            }
            return (None, PowerAction::None);
        }

        if event.event_type == EVENT_KEY {
            self.last_key = Some((source, event));
            self.key_events += 1;
            if source == InputSourceKind::Keys && event.code == KEY_MENU {
                return self.observe_menu(event);
            }
            if source == InputSourceKind::Keys && self.page == UiPage::Home && event.value == 1 {
                let operation = match event.code {
                    KEY_LEFT => Some(ReaderOperation::PreviousPage),
                    KEY_RIGHT => Some(ReaderOperation::NextPage),
                    _ => None,
                };
                if let Some(operation) = operation {
                    self.reader_operation = Some(operation);
                    self.message = match operation {
                        ReaderOperation::PreviousPage => "Previous page".into(),
                        ReaderOperation::NextPage => "Next page".into(),
                        ReaderOperation::Back | ReaderOperation::ReturnToEntryPoint => {
                            unreachable!()
                        }
                    };
                    return (Some(DirtyArea::Full), PowerAction::None);
                }
            }
            if source.is_power() && event.code == KEY_POWER {
                eprintln!(
                    "standalone-test: power event source={} value={} timestamp_us={} mode={}",
                    source.label(),
                    event.value,
                    event.timestamp_micros(),
                    self.mode,
                );
                return self.observe_power(source, event);
            }
            let area = if source.is_power() {
                DirtyArea::Power
            } else {
                DirtyArea::Key
            };
            return (Some(area), PowerAction::None);
        }

        (None, PowerAction::None)
    }

    fn touch_release_dirty(&self) -> DirtyArea {
        if self.page == UiPage::DisplayTest {
            return DirtyArea::Full;
        }
        if self.page == UiPage::Home && self.touch_y >= display::STATUS_BAR_HEIGHT as i32 {
            // The reader will refine this into a page-turn tone or a retained
            // interaction message. Keep the status bar out of the initial
            // semantic hint so a normal page turn can use DU.
            DirtyArea::Interaction
        } else {
            // Status-bar/details actions change application chrome and need a
            // complete quality redraw.
            DirtyArea::Full
        }
    }

    fn poll_menu_hold(&mut self) -> Option<DirtyArea> {
        let due = self
            .menu_pressed_at
            .map(|started| {
                !self.menu_hold_triggered
                    && started.elapsed() >= Duration::from_micros(MENU_HOLD_MICROS)
            })
            .unwrap_or(false);
        due.then(|| self.trigger_menu_redraw())
    }

    fn observe_menu(&mut self, event: RawEvent) -> (Option<DirtyArea>, PowerAction) {
        match event.value {
            1 => {
                if self.menu_press_us.is_none() {
                    self.menu_press_us = Some(event.timestamp_micros());
                    self.menu_pressed_at = Some(Instant::now());
                    self.menu_hold_triggered = false;
                    self.message = "Menu held".into();
                }
                (Some(DirtyArea::Key), PowerAction::None)
            }
            2 => {
                let due = !self.menu_hold_triggered
                    && self
                        .menu_press_us
                        .map(|started| {
                            event.timestamp_micros().saturating_sub(started) >= MENU_HOLD_MICROS
                        })
                        .unwrap_or(false);
                if due {
                    (Some(self.trigger_menu_redraw()), PowerAction::None)
                } else {
                    self.message = "Menu repeat".into();
                    (Some(DirtyArea::Key), PowerAction::None)
                }
            }
            0 => {
                let Some(start) = self.menu_press_us.take() else {
                    self.menu_pressed_at = None;
                    self.menu_hold_triggered = false;
                    self.message = "Menu released".into();
                    return (Some(DirtyArea::Key), PowerAction::None);
                };
                let duration = event.timestamp_micros().saturating_sub(start);
                self.menu_pressed_at = None;
                let already_triggered = self.menu_hold_triggered;
                self.menu_hold_triggered = false;
                if !already_triggered && duration >= MENU_HOLD_MICROS {
                    (Some(self.trigger_menu_redraw()), PowerAction::None)
                } else if !already_triggered && self.page == UiPage::DisplayTest {
                    self.page = UiPage::Details;
                    self.message = "Returned to Details / Settings".into();
                    (Some(DirtyArea::Full), PowerAction::None)
                } else if !already_triggered && self.page == UiPage::Home {
                    self.reader_operation = Some(ReaderOperation::Back);
                    self.message = "Reader back".into();
                    (Some(DirtyArea::Full), PowerAction::None)
                } else {
                    self.message = "Menu released".into();
                    (Some(DirtyArea::Key), PowerAction::None)
                }
            }
            _ => (Some(DirtyArea::Key), PowerAction::None),
        }
    }

    fn trigger_menu_redraw(&mut self) -> DirtyArea {
        self.menu_hold_triggered = true;
        self.message = "Menu hold - full redraw".into();
        eprintln!(
            "standalone-test: menu hold reached {}ms; requesting full {} redraw",
            MENU_HOLD_MICROS / 1_000,
            WaveformMode::Gc16.label()
        );
        DirtyArea::Full
    }

    fn activate_tap(&mut self) -> PowerAction {
        if self.page == UiPage::DisplayTest {
            return PowerAction::None;
        }
        let y = self.touch_y;
        if y < display::STATUS_BAR_HEIGHT as i32 {
            self.page = match self.page {
                UiPage::Home => UiPage::Details,
                UiPage::Details => UiPage::Home,
                UiPage::DisplayTest => {
                    unreachable!("display-test taps return before the status bar")
                }
            };
            self.message = match self.page {
                UiPage::Home => "Returned to reading".into(),
                UiPage::Details => "Details open".into(),
                UiPage::DisplayTest => {
                    unreachable!("display-test taps return before the status bar")
                }
            };
            return PowerAction::None;
        }
        if self.page != UiPage::Details {
            self.reader_tap = Some(Point::new(self.touch_x, self.touch_y));
            return PowerAction::None;
        }
        let within_action_x = self.touch_x >= display::DETAILS_ACTION_MARGIN as i32
            && self.touch_x
                < (display::SCREEN_WIDTH.saturating_sub(display::DETAILS_ACTION_MARGIN)) as i32;
        if !within_action_x {
            return PowerAction::None;
        }

        let within_sync_row = y >= display::DETAILS_SYNC_TOP as i32
            && y < (display::DETAILS_SYNC_TOP + display::DETAILS_ACTION_HEIGHT) as i32;
        if within_sync_row {
            self.sync_requested = true;
            self.message = "Sync requested".into();
            return PowerAction::None;
        }

        let within_entry_point_row = y >= display::DETAILS_RETURN_ENTRY_TOP as i32
            && y < (display::DETAILS_RETURN_ENTRY_TOP + display::DETAILS_ACTION_HEIGHT) as i32;
        if within_entry_point_row {
            self.reader_operation = Some(ReaderOperation::ReturnToEntryPoint);
            self.message = "Returning to entry point".into();
            return PowerAction::None;
        }

        let within_display_test_row = y >= display::DETAILS_DISPLAY_TEST_TOP as i32
            && y < (display::DETAILS_DISPLAY_TEST_TOP + display::DETAILS_ACTION_HEIGHT) as i32;
        if within_display_test_row {
            self.page = UiPage::DisplayTest;
            self.message = "Display test open".into();
            return PowerAction::None;
        }

        let within_reboot_row = y >= display::DETAILS_REBOOT_TOP as i32
            && y < (display::DETAILS_REBOOT_TOP + display::DETAILS_ACTION_HEIGHT) as i32;
        if within_reboot_row {
            self.message = "Reboot requested".into();
            return PowerAction::Reboot;
        }

        let within_power_off_row = y >= display::DETAILS_POWER_OFF_TOP as i32
            && y < (display::DETAILS_POWER_OFF_TOP + display::DETAILS_ACTION_HEIGHT) as i32;
        if within_power_off_row {
            self.message = "Power off requested".into();
            return PowerAction::PowerOff;
        }

        let within_back_row = y >= display::DETAILS_BACK_TOP as i32
            && y < (display::DETAILS_BACK_TOP + display::DETAILS_ACTION_HEIGHT) as i32;
        if within_back_row {
            self.page = UiPage::Home;
            self.message = "Returned to reading".into();
        }
        PowerAction::None
    }

    fn observe_power(
        &mut self,
        source: InputSourceKind,
        event: RawEvent,
    ) -> (Option<DirtyArea>, PowerAction) {
        if let Some(deadline) = self.ignore_power_until {
            if Instant::now() < deadline {
                if event.value == 0 {
                    self.ignore_power_until = None;
                }
                self.message = "Wake power ignored".into();
                return (Some(DirtyArea::Power), PowerAction::None);
            }
            self.ignore_power_until = None;
        }
        match event.value {
            1 => {
                if self.power_press_us.is_none() {
                    self.power_press_us = Some(event.timestamp_micros());
                }
                self.message = "Power held".into();
                (Some(DirtyArea::Power), PowerAction::None)
            }
            0 => {
                let duration = self
                    .power_press_us
                    .take()
                    .map(|start| event.timestamp_micros().saturating_sub(start))
                    .unwrap_or_default();
                self.last_power_duration_ms = Some(duration / 1_000);
                if duration >= LONG_PRESS_MICROS {
                    self.message = "Long power - reboot".into();
                    eprintln!(
                        "standalone-test: power release source={} duration_ms={} action=REBOOT",
                        source.label(),
                        duration / 1_000
                    );
                    (Some(DirtyArea::Power), PowerAction::Reboot)
                } else {
                    self.message = "Short power - sleep".into();
                    eprintln!(
                        "standalone-test: power release source={} duration_ms={} action=SLEEP",
                        source.label(),
                        duration / 1_000
                    );
                    (Some(DirtyArea::Power), PowerAction::Sleep)
                }
            }
            2 => {
                self.message = "Power repeat".into();
                (Some(DirtyArea::Power), PowerAction::None)
            }
            _ => (Some(DirtyArea::Power), PowerAction::None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        display, BundleHandoff, DirtyArea, InputSourceKind, PageTone, Point, ReaderOperation,
        RefreshReason, SuspendMode, SyncEvent, UiPage, UiState, ABS_MT_POSITION_X,
        ABS_MT_POSITION_Y, ABS_MT_TOUCH_MAJOR, ABS_MT_TRACKING_ID, ABS_X, ABS_Y, BTN_TOUCH,
        EVENT_ABS, EVENT_KEY, EVENT_SYN, KEY_LEFT, KEY_MENU, KEY_RIGHT, SYN_REPORT,
    };
    use crate::input::RawEvent;
    use std::time::{Duration, Instant};

    fn event(code: u16, value: i32, timestamp_micros: u64) -> RawEvent {
        RawEvent {
            sec: (timestamp_micros / 1_000_000) as u32,
            usec: (timestamp_micros % 1_000_000) as u32,
            event_type: EVENT_KEY,
            code,
            value,
        }
    }

    #[test]
    fn parses_supported_suspend_modes() {
        assert_eq!(SuspendMode::parse("standby").unwrap(), SuspendMode::EInk);
        assert_eq!(SuspendMode::parse("mem").unwrap(), SuspendMode::Normal);
    }

    #[test]
    fn rejects_unknown_suspend_mode() {
        assert!(SuspendMode::parse("on").is_err());
    }

    #[test]
    fn top_bar_tap_opens_and_closes_details() {
        let mut state = UiState::new();
        state.touch_x = 100;
        state.touch_y = 20;
        state.touch_down = true;

        let (_, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.page, UiPage::Details);

        state.touch_down = true;
        let _ = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 2_000_000));
        assert_eq!(state.page, UiPage::Home);
    }

    #[test]
    fn content_tap_is_deferred_to_the_markdown_reader() {
        let mut state = UiState::new();
        state.touch_x = 500;
        state.touch_y = display::CONTENT_TOP as i32 + 80;
        state.touch_down = true;

        let (dirty, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));

        assert_eq!(action, super::PowerAction::None);
        assert_eq!(dirty, Some(DirtyArea::Interaction));
        assert_eq!(state.take_reader_tap(), Some(Point::new(500, 156)));
        assert_eq!(state.page, UiPage::Home);
    }

    #[test]
    fn active_mode_is_hidden_from_status_bar() {
        let state = UiState::new();
        let lines = super::screen_lines(&state, true);
        assert_eq!(lines[0].split('|').nth(4), Some(""));
    }

    #[test]
    fn document_and_status_dirty_areas_select_their_device_side_reasons() {
        assert_eq!(
            DirtyArea::PageTurn(PageTone::Monochrome).reason(UiPage::Home),
            RefreshReason::PageTurn(PageTone::Monochrome)
        );
        assert_eq!(
            DirtyArea::Status.reason(UiPage::Home),
            RefreshReason::StatusBar
        );
        assert_eq!(
            DirtyArea::Status.reason(UiPage::Details),
            RefreshReason::Transient
        );
        assert_eq!(
            DirtyArea::Status.merge(DirtyArea::PageTurn(PageTone::Monochrome)),
            DirtyArea::Full
        );
    }

    #[test]
    fn menu_hold_requests_full_redraw_at_one_second() {
        let mut state = UiState::new();

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_MENU, 1, 1_000_000));
        assert_eq!(dirty, Some(DirtyArea::Key));
        assert_eq!(action, super::PowerAction::None);

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_MENU, 0, 2_000_000));
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(action, super::PowerAction::None);
        assert!(state.menu_press_us.is_none());
    }

    #[test]
    fn short_menu_press_requests_reader_back() {
        let mut state = UiState::new();
        state.observe(InputSourceKind::Keys, event(KEY_MENU, 1, 1_000_000));

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_MENU, 0, 1_999_999));
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.take_reader_operation(), Some(ReaderOperation::Back));
    }

    #[test]
    fn page_buttons_queue_one_reader_page_operation_on_press() {
        let mut state = UiState::new();

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_LEFT, 1, 1_000_000));
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(
            state.take_reader_operation(),
            Some(ReaderOperation::PreviousPage)
        );

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_LEFT, 2, 1_000_001));
        assert_eq!(dirty, Some(DirtyArea::Key));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.take_reader_operation(), None);

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_RIGHT, 1, 1_000_002));
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(
            state.take_reader_operation(),
            Some(ReaderOperation::NextPage)
        );
    }

    #[test]
    fn page_buttons_and_reader_back_do_not_leave_details_page() {
        let mut state = UiState::new();
        state.page = UiPage::Details;

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_RIGHT, 1, 1_000_000));
        assert_eq!(dirty, Some(DirtyArea::Key));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.take_reader_operation(), None);

        state.observe(InputSourceKind::Keys, event(KEY_MENU, 1, 2_000_000));
        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_MENU, 0, 2_100_000));
        assert_eq!(dirty, Some(DirtyArea::Key));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.take_reader_operation(), None);
    }

    #[test]
    fn menu_hold_timer_triggers_once_without_key_repeat() {
        let mut state = UiState::new();
        state.observe(InputSourceKind::Keys, event(KEY_MENU, 1, 1_000_000));
        state.menu_pressed_at = Some(Instant::now() - Duration::from_secs(1));

        assert_eq!(state.poll_menu_hold(), Some(DirtyArea::Full));
        assert_eq!(state.poll_menu_hold(), None);
    }

    #[test]
    fn details_action_taps_return_power_actions() {
        let mut state = UiState::new();
        state.page = UiPage::Details;
        state.touch_down = true;
        state.touch_x = 100;
        state.touch_y = super::display::DETAILS_REBOOT_TOP as i32 + 10;
        let (_, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));
        assert_eq!(action, super::PowerAction::Reboot);

        state.touch_down = true;
        state.touch_x = 400;
        state.touch_y = super::display::DETAILS_POWER_OFF_TOP as i32 + 10;
        let (_, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 2_000_000));
        assert_eq!(action, super::PowerAction::PowerOff);

        state.touch_down = true;
        state.touch_y = super::display::DETAILS_BACK_TOP as i32 + 10;
        let (_, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 3_000_000));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.page, UiPage::Home);

        state.page = UiPage::Details;
        state.touch_down = true;
        state.touch_x = 10;
        state.touch_y = super::display::DETAILS_BACK_TOP as i32 + 10;
        let (_, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 4_000_000));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.page, UiPage::Details);
    }

    #[test]
    fn details_action_tap_boundaries_are_disjoint() {
        let tap = |y: usize| {
            let mut state = UiState::new();
            state.page = UiPage::Details;
            state.touch_down = true;
            state.touch_x = 100;
            state.touch_y = y as i32;
            let (_, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));
            (action, state.page)
        };

        assert_eq!(
            tap(display::DETAILS_REBOOT_TOP),
            (super::PowerAction::Reboot, UiPage::Details)
        );
        assert_eq!(
            tap(display::DETAILS_REBOOT_TOP + display::DETAILS_ACTION_HEIGHT - 1),
            (super::PowerAction::Reboot, UiPage::Details)
        );
        assert_eq!(
            tap(display::DETAILS_POWER_OFF_TOP),
            (super::PowerAction::PowerOff, UiPage::Details)
        );
        assert_eq!(
            tap(display::DETAILS_POWER_OFF_TOP + display::DETAILS_ACTION_HEIGHT - 1),
            (super::PowerAction::PowerOff, UiPage::Details)
        );
        assert_eq!(
            tap(display::DETAILS_BACK_TOP),
            (super::PowerAction::None, UiPage::Home)
        );
        assert_eq!(
            tap(display::DETAILS_BACK_TOP + display::DETAILS_ACTION_HEIGHT - 1),
            (super::PowerAction::None, UiPage::Home)
        );
    }

    #[test]
    fn details_display_test_tap_opens_calibration_screen() {
        let mut state = UiState::new();
        state.page = UiPage::Details;
        state.touch_down = true;
        state.touch_x = 100;
        state.touch_y = super::display::DETAILS_DISPLAY_TEST_TOP as i32 + 10;

        let (dirty, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));

        assert_eq!(action, super::PowerAction::None);
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(state.page, UiPage::DisplayTest);
    }

    #[test]
    fn details_sync_and_entry_point_actions_are_available() {
        let mut state = UiState::new();
        state.page = UiPage::Details;
        state.touch_down = true;
        state.touch_x = 100;
        state.touch_y = super::display::DETAILS_SYNC_TOP as i32 + 10;
        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));
        assert!(state.take_sync_request());

        state.touch_down = true;
        state.touch_y = super::display::DETAILS_RETURN_ENTRY_TOP as i32 + 10;
        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_001));
        assert_eq!(
            state.take_reader_operation(),
            Some(ReaderOperation::ReturnToEntryPoint)
        );
    }

    #[test]
    fn idle_sync_uses_the_configured_quiet_period() {
        let mut state = UiState::new();
        let interval = Duration::from_secs(900);
        state.last_sync_completion = Instant::now() - interval;
        state.last_activity = Instant::now() - interval;

        assert!(state.should_start_idle_sync(interval));

        state.page = UiPage::Details;
        assert!(state.should_start_idle_sync(interval));
    }

    #[test]
    fn idle_sync_waits_after_failure_and_skips_display_test() {
        let mut state = UiState::new();
        let interval = Duration::from_secs(900);
        state.sync_started();
        // Model an attempt that started at least one interval ago.
        state.last_sync_completion = Instant::now() - interval;
        state.last_activity = Instant::now() - interval;
        state.apply_sync_event(SyncEvent::Finished(Err("network loss".into())));

        assert!(!state.sync_active);
        assert!(!state.bundle_ready);
        assert_eq!(state.last_sync_failure.as_deref(), Some("network loss"));
        assert!(!state.should_start_idle_sync(interval));

        state.last_sync_completion = Instant::now() - interval;
        state.last_activity = Instant::now() - interval;
        state.page = UiPage::DisplayTest;
        assert!(!state.should_start_idle_sync(interval));
    }

    #[test]
    fn updated_and_cleared_syncs_both_schedule_idle_reader_handoffs() {
        let mut state = UiState::new();
        state.sync_started();
        state.apply_sync_event(SyncEvent::Finished(Ok(super::sync::SyncOutcome::Cleared {
            revision: prs_sync_protocol::InboxRevision::new(4),
        })));

        assert!(!state.sync_active);
        assert!(state.bundle_ready);
        assert_eq!(state.pending_handoff, Some(BundleHandoff::Cleared));
        assert_eq!(state.message, "Library cleared");

        state.sync_started();
        state.apply_sync_event(SyncEvent::Finished(Ok(super::sync::SyncOutcome::Updated {
            revision: prs_sync_protocol::InboxRevision::new(5),
            entry_point: "index.md".into(),
        })));
        assert!(state.bundle_ready);
        assert_eq!(state.pending_handoff, Some(BundleHandoff::Updated));
    }

    #[test]
    fn short_menu_press_returns_from_display_test_to_details() {
        let mut state = UiState::new();
        state.page = UiPage::DisplayTest;
        state.observe(InputSourceKind::Keys, event(KEY_MENU, 1, 1_000_000));

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_MENU, 0, 1_100_000));

        assert_eq!(action, super::PowerAction::None);
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(state.page, UiPage::Details);
        assert_eq!(state.take_reader_operation(), None);
    }

    #[test]
    fn legacy_touch_axes_are_normalized_and_tracking_release_activates_tap() {
        let mut state = UiState::new();
        state.page = UiPage::Details;

        let mut axis_x = event(ABS_X, 770, 1_000_000);
        axis_x.event_type = EVENT_ABS;
        let mut axis_y = event(ABS_Y, 300, 1_000_001);
        axis_y.event_type = EVENT_ABS;
        let mut tracking_down = event(ABS_MT_TRACKING_ID, 7, 1_000_002);
        tracking_down.event_type = EVENT_ABS;
        let mut tracking_up = event(ABS_MT_TRACKING_ID, -1, 1_000_003);
        tracking_up.event_type = EVENT_ABS;
        let mut syn = event(SYN_REPORT, 0, 1_000_004);
        syn.event_type = EVENT_SYN;

        state.observe(InputSourceKind::Touch, axis_x);
        state.observe(InputSourceKind::Touch, axis_y);
        state.observe(InputSourceKind::Touch, tracking_down);
        state.observe(InputSourceKind::Touch, tracking_up);
        let (_, action) = state.observe(InputSourceKind::Touch, syn);

        assert_eq!(state.touch_x, 300);
        assert_eq!(state.touch_y, 770);
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.page, UiPage::Home);
    }

    #[test]
    fn touch_major_release_activates_tap_on_syn_report() {
        let mut state = UiState::new();
        state.page = UiPage::Details;

        let mut position_y = event(ABS_MT_POSITION_Y, 770, 1_000_000);
        position_y.event_type = EVENT_ABS;
        let mut position_x = event(ABS_MT_POSITION_X, 300, 1_000_001);
        position_x.event_type = EVENT_ABS;
        let mut press = event(ABS_MT_TOUCH_MAJOR, 12, 1_000_002);
        press.event_type = EVENT_ABS;
        let mut release = event(ABS_MT_TOUCH_MAJOR, 0, 1_000_003);
        release.event_type = EVENT_ABS;
        let mut syn = event(SYN_REPORT, 0, 1_000_004);
        syn.event_type = EVENT_SYN;

        state.observe(InputSourceKind::Touch, position_y);
        state.observe(InputSourceKind::Touch, position_x);
        state.observe(InputSourceKind::Touch, press);
        state.observe(InputSourceKind::Touch, release);
        let (_, action) = state.observe(InputSourceKind::Touch, syn);

        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.page, UiPage::Home);
    }
}
