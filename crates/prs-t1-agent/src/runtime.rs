use crate::framebuffer::{DisplayRegion, NativeDisplay, WaveformMode};
use crate::input::{EventReader, RawEvent};
use crate::orientation::ReaderOrientation;
use crate::preferences::{next_font_scale_percent, ReaderPreferences};
use crate::refresh::{PageTone, RefreshPolicy, RefreshReason};
use crate::{display, reader, sync};
use embedded_graphics::geometry::Point;
use prs_markdown::reader::ReaderEvent;
use prs_markdown::ReaderControllerError;
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
const KEY_HOME: u16 = 102;
const KEY_BACK: u16 = 158;
// The first key device (/dev/input/event0) reports the physical menu button
// as "Unknown" code 357 (the diagnostic label is E0 Unknown C357).
const KEY_MENU: u16 = 357;
const UI_HISTORY_LIMIT: usize = 8;
const LONG_PRESS_MICROS: u64 = 2_000_000;
const MENU_HOLD_MICROS: u64 = 1_000_000;
const WAKE_LOCK_NAME: &str = "prs-t1-native-test";
const POWER_STATE_HELPER: &str = "/data/local/tmp/prs-t1-power-state";
const FRAMEWORK_STOP_TIMEOUT_SECONDS: u32 = 15;
const DEFAULT_SLEEP_INACTIVITY_SECONDS: u64 = 5 * 60;
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
    // The T1's old Android/Bionic framebuffer driver requires the first
    // writable mapping before this environment/configuration parse. Keep the
    // display open first so a sleep-timeout override cannot make mmap fail.
    let requested_orientation = ReaderOrientation::default();
    let mut display = NativeDisplay::open_with_orientation(path, requested_orientation)
        .map_err(|error| display_error("open native display", error))?;
    // Preserve the five-minute default and the test override, but do not move
    // this parse above NativeDisplay::open() without physical T1 validation.
    let sleep_inactivity_timeout = sleep_inactivity_timeout()?;
    // Read durable presentation settings only after the first writable mmap.
    // NativeDisplay retains that mapping while applying the saved rotation.
    let mut preferences = ReaderPreferences::load();
    if preferences.orientation != display.orientation() {
        display
            .set_orientation(preferences.orientation)
            .map_err(|error| display_error("apply saved display orientation", error))?;
    }
    let orientation = display.orientation();
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
    let mut markdown_reader = reader::T1Reader::open_with_library_root_and_layout(
        reader_config,
        reader::reader_layout_for_display(display.width(), display.height())
            .with_font_scale_percent(preferences.font_scale_percent),
        sync_task.library_root(),
    )
    .map_err(|error| display_error("open development Markdown reader", error))?;
    let mut state = UiState::with_preferences(ReaderPreferences {
        orientation,
        ..preferences
    });
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
        &mut state,
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
        'input: for source in &mut inputs.sources {
            loop {
                let Some(event) = source.reader.read_one()? else {
                    break;
                };
                if dismisses_external_link_overlay(source.kind, event) {
                    if markdown_reader.clear_external_link_overlay() {
                        redraw_area = Some(match redraw_area {
                            Some(existing) => existing.merge(DirtyArea::ExternalLinkOverlay),
                            None => DirtyArea::ExternalLinkOverlay,
                        });
                    }
                }
                let (dirty, event_action) = state.observe(source.kind, event);
                let immediate_feedback = matches!(dirty, Some(DirtyArea::Action(_)));
                if let Some(dirty) = dirty {
                    redraw_area = Some(match redraw_area {
                        Some(existing) => existing.merge(dirty),
                        None => dirty,
                    });
                }
                if immediate_feedback {
                    break 'input;
                }
                if event_action != PowerAction::None {
                    action = event_action;
                }
                if let Some(operation) = state.take_reader_operation() {
                    let fullscreen_operation =
                        matches!(operation, ReaderOperation::SetFullscreen(_));
                    let font_scale_operation = matches!(
                        operation,
                        ReaderOperation::SetFontScale(_)
                            | ReaderOperation::DecreaseFontScale
                            | ReaderOperation::ResetFontScale
                            | ReaderOperation::IncreaseFontScale
                    );
                    let previous_fullscreen = markdown_reader.fullscreen();
                    let result = match operation {
                        ReaderOperation::PreviousPage => markdown_reader.previous_page(),
                        ReaderOperation::NextPage => markdown_reader.next_page(),
                        ReaderOperation::Back => markdown_reader.back(),
                        ReaderOperation::ReturnToEntryPoint => {
                            markdown_reader.return_to_entry_point()
                        }
                        ReaderOperation::SetFullscreen(fullscreen) => {
                            if markdown_reader.fullscreen() == fullscreen {
                                Ok(ReaderEvent::NoAction)
                            } else {
                                markdown_reader.toggle_fullscreen()
                            }
                        }
                        ReaderOperation::SetFontScale(font_scale_percent) => {
                            markdown_reader.set_font_scale_percent(font_scale_percent)
                        }
                        ReaderOperation::DecreaseFontScale => markdown_reader.decrease_font_size(),
                        ReaderOperation::ResetFontScale => markdown_reader.reset_font_size(),
                        ReaderOperation::IncreaseFontScale => markdown_reader.increase_font_size(),
                    };
                    let result_succeeded = result.is_ok();
                    let mut dirty = apply_reader_result(&mut state, &mut markdown_reader, result);
                    if fullscreen_operation {
                        state.fullscreen = markdown_reader.fullscreen();
                        if state.fullscreen != previous_fullscreen {
                            dirty = Some(DirtyArea::Full);
                        }
                    }
                    if font_scale_operation && result_succeeded {
                        let font_scale_percent = markdown_reader.font_scale_percent();
                        if state.font_scale_percent != font_scale_percent {
                            state.font_scale_percent = font_scale_percent;
                            preferences.font_scale_percent = font_scale_percent;
                            if let Err(error) = preferences.save() {
                                state.set_error_feedback(format!(
                                    "Font size changed but could not be saved: {error}"
                                ));
                                eprintln!(
                                    "standalone-test: saving reader font preference failed: {error}"
                                );
                            } else {
                                state
                                    .set_debug_feedback(format!("Font size {font_scale_percent}%"));
                            }
                            dirty = Some(DirtyArea::Full);
                        }
                    }
                    redraw_area = merge_optional_dirty(redraw_area, dirty);
                }
                if let Some(point) = state.take_reader_tap() {
                    let result = markdown_reader.tap(point);
                    let dirty = apply_reader_result(&mut state, &mut markdown_reader, result);
                    redraw_area = merge_optional_dirty(redraw_area, dirty);
                }
                if let Some(target) = state.take_orientation_change() {
                    let dirty = apply_orientation_change(
                        &mut display,
                        &mut markdown_reader,
                        &mut state,
                        target,
                        &mut preferences,
                    );
                    redraw_area = merge_optional_dirty(redraw_area, dirty);
                }
            }
        }

        if start_requested_sync(&mut state, || sync_task.start()).is_some() {
            redraw_area = merge_optional_dirty(redraw_area, Some(state.sync_status_dirty()));
        }
        while let Some(event) = sync_task.try_event() {
            redraw_area = merge_optional_dirty(redraw_area, state.apply_sync_event(event));
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
                        state.set_error_feedback("Bundle ready; cleanup failed");
                        eprintln!("standalone-test: retired bundle cleanup failed: {error}");
                    } else {
                        state.set_debug_feedback(if state.library_empty {
                            "Library cleared"
                        } else {
                            "New bundle ready"
                        });
                    }
                    redraw_area = Some(DirtyArea::Full);
                }
                Err(error) => {
                    state.last_sync_failure = Some(format!("reload current bundle: {error}"));
                    state.set_error_feedback("Bundle handoff failed");
                    eprintln!("standalone-test: current bundle handoff failed: {error}");
                    redraw_area = merge_optional_dirty(redraw_area, Some(DirtyArea::Feedback));
                }
            }
        }

        if action == PowerAction::None
            && state.should_enter_inactivity_sleep(sleep_inactivity_timeout)
        {
            eprintln!(
                "standalone-test: inactivity reached {}s; requesting sleep",
                sleep_inactivity_timeout.as_secs()
            );
            action = PowerAction::Sleep;
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
                state.set_debug_feedback(match action {
                    PowerAction::Reboot => "REBOOT REQUESTED",
                    PowerAction::PowerOff => "POWER OFF REQUESTED",
                    PowerAction::None | PowerAction::Sleep => unreachable!(),
                });
                redraw(
                    &mut display,
                    &mut state,
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
                    &mut state,
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
    state: &mut UiState,
    markdown_reader: &mut reader::T1Reader,
    wake_lock_held: bool,
    area: DirtyArea,
    refresh_policy: &mut RefreshPolicy,
    sync_task: Option<&mut SyncTask>,
) -> io::Result<()> {
    let view = screen_view_model(state, wake_lock_held);
    let lines = screen_lines_for_page(state, &view);
    markdown_reader.set_progress_line_enabled(state.reading_progress);
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
        return finish_redraw(result, state, sync_task);
    }
    if state.sync_active {
        if let Some(approval_url) = state.approval_url.as_deref() {
            return finish_redraw(
                display::draw_authorization_qr_with_plan(
                    display,
                    approval_url,
                    &view.status_bar.to_wire_line(),
                    area.region(display.width(), display.height(), state.page),
                    plan.waveform(),
                    plan.wait_for_completion(),
                    plan.force_refresh(),
                ),
                state,
                sync_task,
            );
        }
    }
    if state.page == UiPage::Home {
        if state.library_empty {
            let result = display::draw_screen_with_feedback(
                display,
                &lines,
                state.visible_feedback(),
                area.region(display.width(), display.height(), state.page),
                plan.waveform(),
                plan.wait_for_completion(),
                plan.force_refresh(),
            );
            if result.is_ok() {
                refresh_policy.record_success(reason, plan);
            }
            return finish_redraw(result, state, sync_task);
        }
        let status_line = lines.first().map(String::as_str).unwrap_or_default();
        let result = markdown_reader.draw(
            display,
            status_line,
            state.visible_feedback(),
            area.region(display.width(), display.height(), state.page),
            plan,
        );
        if result.is_ok() {
            refresh_policy.record_success(reason, plan);
        }
        return finish_redraw(result, state, sync_task);
    }
    let result = match state.pressed_action {
        Some(pressed_action) => display::draw_screen_view_with_pressed_action(
            display,
            &view,
            area.region(display.width(), display.height(), state.page),
            plan.waveform(),
            plan.wait_for_completion(),
            plan.force_refresh(),
            Some(pressed_action),
        ),
        None => display::draw_screen_view(
            display,
            &view,
            area.region(display.width(), display.height(), state.page),
            plan.waveform(),
            plan.wait_for_completion(),
            plan.force_refresh(),
        ),
    };
    if result.is_ok() {
        refresh_policy.record_success(reason, plan);
    }
    finish_redraw(result, state, sync_task)
}

fn finish_redraw(
    result: io::Result<()>,
    state: &mut UiState,
    sync_task: Option<&mut SyncTask>,
) -> io::Result<()> {
    let Err(render_error) = result else {
        return Ok(());
    };
    let Some(sync_task) = sync_task else {
        return Err(render_error);
    };
    match sync_task.cancel() {
        Ok(()) => {
            state.sync_cancelled();
            Err(render_error)
        }
        Err(cleanup_error) => {
            state.sync_cancelled();
            Err(io::Error::new(
                render_error.kind(),
                format!(
                    "display render failed: {render_error}; Wi-Fi cleanup failed: {cleanup_error}"
                ),
            ))
        }
    }
}

fn reader_event_dirty_area(event: &ReaderEvent, tone: PageTone) -> Option<DirtyArea> {
    match event {
        ReaderEvent::PageChanged { .. }
        | ReaderEvent::Navigated { .. }
        | ReaderEvent::Back { .. }
        | ReaderEvent::Forward { .. } => Some(DirtyArea::PageTurn(tone)),
        ReaderEvent::ExternalUrl(_) => Some(DirtyArea::ExternalLinkOverlay),
        ReaderEvent::Asset(_) => Some(DirtyArea::Interaction),
        ReaderEvent::NoAction => None,
        ReaderEvent::Opened { .. } => None,
    }
}

fn dismisses_external_link_overlay(source: InputSourceKind, event: RawEvent) -> bool {
    source == InputSourceKind::Touch || event.event_type == EVENT_KEY
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

fn record_reader_event_feedback(state: &mut UiState, event: &ReaderEvent) {
    state.set_debug_feedback(reader_event_message(event));
}

fn apply_reader_result(
    state: &mut UiState,
    markdown_reader: &mut reader::T1Reader,
    result: Result<ReaderEvent, ReaderControllerError>,
) -> Option<DirtyArea> {
    match result {
        Ok(event) => {
            let previous_feedback = state.feedback_is_visible().map(str::to_owned);
            record_reader_event_feedback(state, &event);
            let dirty = if matches!(&event, ReaderEvent::NoAction) {
                (state.feedback_is_visible().map(str::to_owned) != previous_feedback)
                    .then_some(DirtyArea::Feedback)
            } else {
                reader_event_dirty_area(&event, markdown_reader.current_page_tone()).or_else(|| {
                    matches!(&event, ReaderEvent::Opened { .. }).then_some(DirtyArea::Full)
                })
            };
            dirty
        }
        Err(error) => {
            state.set_error_feedback(format!("Reader error: {error}"));
            markdown_reader.clear_external_link_overlay();
            eprintln!("standalone-test: Markdown operation failed: {error}");
            Some(DirtyArea::Full)
        }
    }
}

fn apply_orientation_change(
    display: &mut NativeDisplay,
    markdown_reader: &mut reader::T1Reader,
    state: &mut UiState,
    target: ReaderOrientation,
    preferences: &mut ReaderPreferences,
) -> Option<DirtyArea> {
    let previous = state.orientation;
    if target == previous {
        return None;
    }

    if let Err(error) = display.set_orientation(target) {
        state.set_error_feedback(format!("Display orientation failed: {error}"));
        eprintln!("standalone-test: display orientation change failed: {error}");
        return Some(DirtyArea::Full);
    }

    if let Err(error) = markdown_reader.set_orientation(target) {
        let rollback = display.set_orientation(previous);
        state.set_error_feedback(format!("Reader orientation failed: {error}"));
        eprintln!(
            "standalone-test: reader orientation change failed: {error}; framebuffer rollback={rollback:?}"
        );
        return Some(DirtyArea::Full);
    }

    state.orientation = target;
    preferences.orientation = target;
    let saved = if let Err(error) = preferences.save() {
        state.set_error_feedback(format!(
            "Orientation changed but could not be saved: {error}"
        ));
        eprintln!("standalone-test: saving reader orientation preference failed: {error}");
        false
    } else {
        true
    };
    if saved {
        state.set_debug_feedback(format!("Orientation {}", target.label()));
    }
    eprintln!(
        "standalone-test: orientation={} framebuffer={}x{} touch-map={}",
        target.label(),
        display.width(),
        display.height(),
        if target.is_landscape() {
            "native"
        } else {
            "swapped"
        },
    );
    Some(DirtyArea::Full)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirtyArea {
    Full,
    Status,
    SyncStatus,
    StatusFeedback,
    Feedback,
    PageTurn(PageTone),
    Interaction,
    ExternalLinkOverlay,
    Action(display::DetailsAction),
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
            (Self::Status, Self::SyncStatus) | (Self::SyncStatus, Self::Status) => Self::SyncStatus,
            (Self::Status, Self::StatusFeedback) | (Self::StatusFeedback, Self::Status) => {
                Self::StatusFeedback
            }
            (Self::Status, Self::Feedback) | (Self::Feedback, Self::Status) => Self::StatusFeedback,
            (Self::SyncStatus, Self::StatusFeedback) | (Self::StatusFeedback, Self::SyncStatus) => {
                Self::StatusFeedback
            }
            (Self::PageTurn(tone), Self::Interaction)
            | (Self::Interaction, Self::PageTurn(tone)) => Self::PageTurn(tone),
            (Self::SyncStatus, Self::Feedback) | (Self::Feedback, Self::SyncStatus) => {
                Self::StatusFeedback
            }
            (Self::SyncStatus, Self::Action(_)) | (Self::Action(_), Self::SyncStatus) => {
                Self::SyncStatus
            }
            (Self::Feedback, Self::Action(_)) | (Self::Action(_), Self::Feedback) => {
                Self::Interaction
            }
            (Self::Feedback, Self::Interaction) | (Self::Interaction, Self::Feedback) => {
                Self::Interaction
            }
            (Self::ExternalLinkOverlay, Self::Feedback)
            | (Self::Feedback, Self::ExternalLinkOverlay)
            | (Self::ExternalLinkOverlay, Self::Interaction)
            | (Self::Interaction, Self::ExternalLinkOverlay) => Self::Interaction,
            (Self::ExternalLinkOverlay, Self::ExternalLinkOverlay) => Self::ExternalLinkOverlay,
            (Self::Action(left), Self::Action(right)) if left == right => Self::Action(left),
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
            Self::SyncStatus => RefreshReason::StatusBar,
            Self::Status
            | Self::StatusFeedback
            | Self::Interaction
            | Self::Feedback
            | Self::ExternalLinkOverlay
            | Self::Action(_)
            | Self::Touch
            | Self::Key
            | Self::Power => RefreshReason::Transient,
        }
    }

    fn region(self, width: u32, height: u32, page: UiPage) -> DisplayRegion {
        let content_region = || {
            DisplayRegion::new(
                0,
                display::STATUS_BAR_HEIGHT as u32,
                width,
                height.saturating_sub(display::STATUS_BAR_HEIGHT as u32),
            )
        };
        let feedback_region = || {
            DisplayRegion::new(
                0,
                display::STATUS_BAR_HEIGHT as u32,
                width,
                (display::CONTENT_TOP as u32).saturating_sub(display::STATUS_BAR_HEIGHT as u32),
            )
        };
        let region = match self {
            Self::Full => DisplayRegion::full(width, height),
            Self::PageTurn(_) | Self::Interaction => content_region(),
            Self::ExternalLinkOverlay => reader::external_link_overlay_region(width, height),
            Self::Status if page == UiPage::Home => {
                DisplayRegion::new(0, 0, width, display::STATUS_BAR_HEIGHT as u32)
            }
            Self::Status => DisplayRegion::new(0, 0, width, 640),
            Self::SyncStatus if page == UiPage::Home => {
                DisplayRegion::new(0, 0, width, display::STATUS_BAR_HEIGHT as u32)
            }
            Self::SyncStatus => DisplayRegion::new(
                0,
                0,
                width,
                (display::DETAILS_SYNC_TOP + display::DETAILS_ACTION_HEIGHT) as u32,
            ),
            Self::StatusFeedback => DisplayRegion::new(
                0,
                0,
                width,
                if page == UiPage::Home {
                    display::CONTENT_TOP as u32
                } else {
                    (display::DETAILS_SYNC_TOP + display::DETAILS_ACTION_HEIGHT) as u32
                },
            ),
            Self::Feedback => feedback_region(),
            // Home feedback is cleared before every touch/key event. Repaint
            // that band for the diagnostic-only damage hints as well, or the
            // old message can remain visible until a later content redraw.
            Self::Touch if page == UiPage::Home => feedback_region(),
            Self::Key | Self::Power if page == UiPage::Home => feedback_region(),
            // The details page has responsive row geometry in landscape, so
            // repaint its content band for live input diagnostics.
            Self::Touch | Self::Key | Self::Power => DisplayRegion::new(
                0,
                display::STATUS_BAR_HEIGHT as u32,
                width,
                height.saturating_sub(display::STATUS_BAR_HEIGHT as u32),
            ),
            Self::Action(action) => {
                let details_page = match page {
                    UiPage::Details => display::DetailsPage::Menu,
                    UiPage::Reading => display::DetailsPage::Reading,
                    UiPage::Synchronization => display::DetailsPage::Synchronization,
                    UiPage::DeviceDiagnostics => display::DetailsPage::DeviceDiagnostics,
                    UiPage::Home | UiPage::DisplayTest => display::DetailsPage::Legacy,
                };
                display::details_action_region_for_page(
                    details_page,
                    action,
                    width as usize,
                    height as usize,
                )
                .unwrap_or_else(|| DisplayRegion::full(width, height))
            }
        };
        region.bounded(width, height)
    }
}

fn merge_optional_dirty(left: Option<DirtyArea>, right: Option<DirtyArea>) -> Option<DirtyArea> {
    match (left, right) {
        (None, value) | (value, None) => value,
        (Some(left), Some(right)) => Some(left.merge(right)),
    }
}

fn screen_view_model(state: &UiState, wake_lock_held: bool) -> display::UiViewModel {
    let status = &state.status;
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
    let mode_label = if state.sync_active {
        "SYNCING".into()
    } else if state.mode == "ACTIVE" {
        String::new()
    } else {
        pretty_value(state.mode)
    };
    let clock = short_clock();
    let current_date = date_time();

    let status_bar = display::StatusBarViewModel {
        battery: battery_label,
        wifi: pretty_value(&wifi_state),
        usb: pretty_value(usb_connected),
        adb: pretty_value(adb),
        mode: mode_label,
        clock,
    };
    let legacy_rows = || {
        vec![
            display::DetailsRow::Section("Settings".into()),
            display::DetailsRow::Toggle {
                label: "Debug messages".into(),
                enabled: state.debug_messages,
            },
            display::DetailsRow::Toggle {
                label: "Reading progress".into(),
                enabled: state.reading_progress,
            },
            display::DetailsRow::Toggle {
                label: "Fullscreen reader".into(),
                enabled: state.fullscreen,
            },
            display::DetailsRow::Choice {
                label: "Font size".into(),
                value: format!("{}%", state.font_scale_percent),
            },
            display::DetailsRow::Choice {
                label: "Orientation".into(),
                value: state.orientation.label().into(),
            },
            display::DetailsRow::Section("Synchronization".into()),
            display::DetailsRow::Value(format!(
                "Sync {}  Failure {}",
                if state.sync_active { "active" } else { "idle" },
                state.last_sync_failure.as_deref().unwrap_or("none")
            )),
            display::DetailsRow::Section("Power".into()),
            display::DetailsRow::Value(format!(
                "Battery {} {}  Temp {} C",
                percent_label(&battery_level),
                pretty_value(&battery_state),
                temperature
            )),
            display::DetailsRow::Value(format!(
                "Health {}  Voltage {}  AC {} USB {}",
                pretty_value(&uppercase_or_unknown(status.battery.health.as_deref())),
                voltage_label(status.battery.voltage_uv),
                pretty_value(ac),
                pretty_value(usb_power)
            )),
            display::DetailsRow::Section("Connectivity".into()),
            display::DetailsRow::Value(format!(
                "WiFi {} {}  Supplicant {}",
                status.wifi.interface.to_ascii_lowercase(),
                pretty_value(&wifi_state),
                pretty_value(&supplicant)
            )),
            display::DetailsRow::Value(format!(
                "USB {}  Gadget {}  ADB {}  Functions {}",
                pretty_value(usb_connected),
                pretty_value(&uppercase_or_unknown(status.usb.gadget_state.as_deref())),
                pretty_value(adb),
                pretty_value(&uppercase_or_unknown(
                    status.usb.gadget_functions.as_deref()
                ))
            )),
            display::DetailsRow::Section("Storage".into()),
            display::DetailsRow::Value(format!(
                "Data {} KiB",
                number_or_unknown(status.storage.data.available_kib)
            )),
            display::DetailsRow::Value(format!(
                "SD card {} KiB",
                number_or_unknown(status.storage.sdcard.available_kib)
            )),
            display::DetailsRow::Section("Diagnostics".into()),
            display::DetailsRow::Value(format!(
                "System: FB {}  Rotate {}  zygote {}  dispd {}",
                pretty_value(framebuffer),
                number_or_unknown(status.screen.rotate),
                pretty_value(zygote),
                pretty_value(dispd)
            )),
            display::DetailsRow::Value(format!(
                "Runtime: Wake {}  Date {}  Input {} touch {} key  Power {}",
                pretty_value(if wake_lock_held { "yes" } else { "no" }),
                current_date,
                state.touch_events,
                state.key_events,
                state
                    .last_power_duration_ms
                    .map(|duration| format!("{}ms", duration))
                    .unwrap_or_else(|| "none".into())
            )),
        ]
    };
    let (page, title, rows) = match state.page {
        UiPage::Details if state.settings_menu => {
            (display::DetailsPage::Menu, "Settings", Vec::new())
        }
        UiPage::Reading => (
            display::DetailsPage::Reading,
            "Reading",
            vec![
                display::DetailsRow::Choice {
                    label: "Orientation".into(),
                    value: state.orientation.label().into(),
                },
                display::DetailsRow::Toggle {
                    label: "Show status bar".into(),
                    enabled: !state.fullscreen,
                },
                display::DetailsRow::Choice {
                    label: "Font size".into(),
                    value: format!("{}%", state.font_scale_percent),
                },
                display::DetailsRow::Toggle {
                    label: "Reading progress".into(),
                    enabled: state.reading_progress,
                },
            ],
        ),
        UiPage::Synchronization => (
            display::DetailsPage::Synchronization,
            "Synchronization",
            vec![
                display::DetailsRow::Value(format!(
                    "Status {}",
                    if state.sync_active { "Active" } else { "Idle" }
                )),
                display::DetailsRow::Value(format!(
                    "Failure {}",
                    state.last_sync_failure.as_deref().unwrap_or("None")
                )),
            ],
        ),
        UiPage::DeviceDiagnostics => (
            display::DetailsPage::DeviceDiagnostics,
            "Device & diagnostics",
            vec![display::DetailsRow::Toggle {
                label: "Debug messages".into(),
                enabled: state.debug_messages,
            }],
        ),
        UiPage::Details => (
            display::DetailsPage::Legacy,
            "Details / Settings",
            legacy_rows(),
        ),
        UiPage::Home | UiPage::DisplayTest => (
            display::DetailsPage::Legacy,
            "Details / Settings",
            legacy_rows(),
        ),
    };
    display::UiViewModel::new(
        status_bar,
        display::DetailsViewModel::new_page(page, title, rows),
    )
}

fn screen_lines(state: &UiState, wake_lock_held: bool) -> Vec<String> {
    let view = screen_view_model(state, wake_lock_held);
    screen_lines_for_page(state, &view)
}

fn screen_lines_for_page(state: &UiState, view: &display::UiViewModel) -> Vec<String> {
    let header = view.status_bar.to_wire_line();
    if state.page == UiPage::Home {
        return vec![
            header,
            "PRS-T1 Native Shell".into(),
            "Tap status for details".into(),
        ];
    }
    if state.page == UiPage::DisplayTest {
        return vec![header, "Display Test".into()];
    }

    let mut lines = vec![header];
    lines.extend(view.details.to_lines());
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

fn sleep_inactivity_timeout() -> io::Result<Duration> {
    parse_sleep_inactivity_timeout(std::env::var("PRS_T1_SLEEP_INACTIVITY_SECONDS").ok())
}

fn parse_sleep_inactivity_timeout(value: Option<String>) -> io::Result<Duration> {
    let seconds = value
        .as_deref()
        .map(|value| {
            value.parse::<u64>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "PRS_T1_SLEEP_INACTIVITY_SECONDS must be an unsigned integer",
                )
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_SLEEP_INACTIVITY_SECONDS);
    if seconds == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "PRS_T1_SLEEP_INACTIVITY_SECONDS must be greater than zero",
        ));
    }
    Ok(Duration::from_secs(seconds))
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
    if state.sync_active {
        sync_task.cancel()?;
        state.sync_cancelled();
    }
    state.enter_sleep(suspend_mode);
    state.set_debug_feedback(match suspend_mode {
        SuspendMode::EInk => "E-ink standby mode",
        SuspendMode::Normal => "Normal mem mode",
    });
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
    let view = screen_view_model(state, wake_lock.is_held());
    let standby = match state.page {
        UiPage::Home => markdown_reader.render_frame(
            standby_lines
                .first()
                .map(String::as_str)
                .unwrap_or_default(),
            state.visible_feedback(),
            display.width(),
            display.height(),
        )?,
        UiPage::DisplayTest => display::render_interactive_display_test(
            display.width() as usize,
            display.height() as usize,
        ),
        UiPage::Details | UiPage::Reading | UiPage::Synchronization | UiPage::DeviceDiagnostics => {
            display::render_details_settings_frame(
                &view,
                display.width() as usize,
                display.height() as usize,
            )
        }
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

    let woke = state.wake();
    debug_assert!(woke, "sleep_cycle must wake an active sleep state");
    state.refresh_status();
    state.set_debug_feedback(format!("Woke after {suspend_elapsed_ms}ms"));
    state.last_power_duration_ms = None;
    state.ignore_power_until = Some(Instant::now() + Duration::from_secs(2));
    if woke {
        eprintln!("standalone-test: wake transition queued one automatic synchronization");
    }
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
                    state.clear_feedback();
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
    SetFullscreen(bool),
    SetFontScale(u16),
    DecreaseFontScale,
    ResetFontScale,
    IncreaseFontScale,
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

fn start_requested_sync<F>(state: &mut UiState, start: F) -> Option<SyncTrigger>
where
    F: FnOnce() -> bool,
{
    let trigger = state.take_sync_trigger()?;
    if start() {
        state.sync_started();
        Some(trigger)
    } else {
        None
    }
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
    Reading,
    Synchronization,
    DeviceDiagnostics,
    DisplayTest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncTrigger {
    Wake,
    Manual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Feedback {
    Error(String),
    Debug(String),
}

struct UiState {
    orientation: ReaderOrientation,
    orientation_change: Option<ReaderOrientation>,
    font_scale_percent: u16,
    page: UiPage,
    settings_menu: bool,
    ui_history: Vec<UiPage>,
    mode: &'static str,
    feedback: Option<Feedback>,
    debug_messages: bool,
    reading_progress: bool,
    fullscreen: bool,
    reader_tap: Option<Point>,
    reader_operation: Option<ReaderOperation>,
    touch_seen: bool,
    touch_raw_x: i32,
    touch_raw_y: i32,
    touch_x: i32,
    touch_y: i32,
    touch_down: bool,
    touch_action: Option<display::DetailsAction>,
    pressed_action: Option<display::DetailsAction>,
    touch_action_initialized: bool,
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
    wake_sync_requested: bool,
    approval_url: Option<String>,
    pub(crate) bundle_ready: bool,
    pending_handoff: Option<BundleHandoff>,
    library_empty: bool,
    last_sync_failure: Option<String>,
    last_activity: Instant,
}

impl UiState {
    fn new() -> Self {
        Self::with_preferences(ReaderPreferences::default())
    }

    fn with_orientation(orientation: ReaderOrientation) -> Self {
        Self::with_preferences(ReaderPreferences {
            orientation,
            ..ReaderPreferences::default()
        })
    }

    fn with_preferences(preferences: ReaderPreferences) -> Self {
        Self {
            orientation: preferences.orientation,
            orientation_change: None,
            font_scale_percent: preferences.font_scale_percent,
            page: UiPage::Home,
            settings_menu: false,
            ui_history: Vec::new(),
            mode: "ACTIVE",
            feedback: None,
            debug_messages: false,
            reading_progress: true,
            fullscreen: false,
            reader_tap: None,
            reader_operation: None,
            touch_seen: false,
            touch_raw_x: 0,
            touch_raw_y: 0,
            touch_x: 0,
            touch_y: 0,
            touch_down: false,
            touch_action: None,
            pressed_action: None,
            touch_action_initialized: false,
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
            wake_sync_requested: false,
            approval_url: None,
            bundle_ready: false,
            pending_handoff: None,
            library_empty: false,
            last_sync_failure: None,
            last_activity: Instant::now(),
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

    fn take_orientation_change(&mut self) -> Option<ReaderOrientation> {
        self.orientation_change.take()
    }

    fn screen_width(&self) -> usize {
        self.orientation.viewport().width as usize
    }

    fn screen_height(&self) -> usize {
        self.orientation.viewport().height as usize
    }

    fn is_settings_page(&self) -> bool {
        matches!(
            self.page,
            UiPage::Details | UiPage::Reading | UiPage::Synchronization | UiPage::DeviceDiagnostics
        )
    }

    fn details_page(&self) -> display::DetailsPage {
        match self.page {
            UiPage::Details if self.settings_menu => display::DetailsPage::Menu,
            UiPage::Details => display::DetailsPage::Legacy,
            UiPage::Reading => display::DetailsPage::Reading,
            UiPage::Synchronization => display::DetailsPage::Synchronization,
            UiPage::DeviceDiagnostics => display::DetailsPage::DeviceDiagnostics,
            UiPage::Home | UiPage::DisplayTest => display::DetailsPage::Legacy,
        }
    }

    fn open_ui_page(&mut self, page: UiPage, message: &'static str) {
        if self.page == page {
            return;
        }
        self.ui_history.push(self.page);
        if self.ui_history.len() > UI_HISTORY_LIMIT {
            self.ui_history.remove(0);
        }
        self.page = page;
        self.settings_menu = page == UiPage::Details;
        self.set_debug_feedback(message);
    }

    fn return_to_reader(&mut self, message: &'static str) {
        self.page = UiPage::Home;
        self.settings_menu = false;
        self.ui_history.clear();
        self.set_debug_feedback(message);
    }

    fn home_button(&mut self) -> Option<DirtyArea> {
        if self.page == UiPage::Home {
            self.ui_history.clear();
            self.reader_operation = Some(ReaderOperation::ReturnToEntryPoint);
            self.set_debug_feedback("Reader entry point");
            None
        } else {
            self.return_to_reader("Returned to reading");
            Some(DirtyArea::Full)
        }
    }

    fn back_button(&mut self) -> Option<DirtyArea> {
        match self.page {
            UiPage::Home => {
                self.reader_operation = Some(ReaderOperation::Back);
                self.set_debug_feedback("Reader back");
                None
            }
            UiPage::Details
            | UiPage::Reading
            | UiPage::Synchronization
            | UiPage::DeviceDiagnostics
            | UiPage::DisplayTest => {
                let previous = self.ui_history.pop().unwrap_or(match self.page {
                    UiPage::Details => UiPage::Home,
                    UiPage::Reading | UiPage::Synchronization | UiPage::DeviceDiagnostics => {
                        UiPage::Details
                    }
                    UiPage::DisplayTest => UiPage::Details,
                    UiPage::Home => unreachable!(),
                });
                self.page = previous;
                self.settings_menu = self.page == UiPage::Details && !self.ui_history.is_empty();
                if self.page == UiPage::Home {
                    self.ui_history.clear();
                }
                self.set_debug_feedback(match self.page {
                    UiPage::Home => "Returned to reading",
                    UiPage::Details => "Returned to Details / Settings",
                    UiPage::Reading => "Returned to Reading",
                    UiPage::Synchronization => "Returned to Synchronization",
                    UiPage::DeviceDiagnostics => "Returned to Device & diagnostics",
                    UiPage::DisplayTest => unreachable!(),
                });
                Some(DirtyArea::Full)
            }
        }
    }

    fn take_sync_trigger(&mut self) -> Option<SyncTrigger> {
        if std::mem::take(&mut self.wake_sync_requested) {
            Some(SyncTrigger::Wake)
        } else if std::mem::take(&mut self.sync_requested) {
            Some(SyncTrigger::Manual)
        } else {
            None
        }
    }

    fn request_manual_sync(&mut self) {
        self.sync_requested = true;
        self.set_debug_feedback("Sync requested");
    }

    fn visible_feedback(&self) -> Option<&str> {
        match self.feedback.as_ref() {
            Some(Feedback::Error(text)) => Some(text),
            Some(Feedback::Debug(text)) if self.debug_messages => Some(text),
            Some(Feedback::Debug(_)) | None => None,
        }
    }

    fn feedback_is_visible(&self) -> Option<&str> {
        if self.page == UiPage::Home && !(self.sync_active && self.approval_url.is_some()) {
            self.visible_feedback()
        } else {
            None
        }
    }

    fn set_error_feedback(&mut self, text: impl Into<String>) {
        self.feedback = Some(Feedback::Error(text.into()));
    }

    fn set_debug_feedback(&mut self, text: impl Into<String>) {
        self.feedback = Some(Feedback::Debug(text.into()));
    }

    fn clear_feedback(&mut self) {
        self.feedback = None;
    }

    fn sync_started(&mut self) {
        self.sync_active = true;
        self.approval_url = None;
        self.set_debug_feedback("Synchronizing…");
    }

    fn sync_status_dirty(&self) -> DirtyArea {
        if self.debug_messages {
            DirtyArea::StatusFeedback
        } else {
            DirtyArea::SyncStatus
        }
    }

    fn sync_cancelled(&mut self) {
        self.sync_active = false;
        self.approval_url = None;
        self.set_error_feedback("Synchronization cancelled");
    }

    fn enter_sleep(&mut self, suspend_mode: SuspendMode) {
        self.mode = "SLEEPING";
        self.wake_sync_requested = false;
        self.set_debug_feedback(match suspend_mode {
            SuspendMode::EInk => "E-ink standby mode",
            SuspendMode::Normal => "Normal mem mode",
        });
    }

    fn wake(&mut self) -> bool {
        if self.mode != "SLEEPING" {
            return false;
        }
        self.mode = "ACTIVE";
        self.last_activity = Instant::now();
        self.wake_sync_requested = true;
        true
    }

    fn apply_sync_event(&mut self, event: SyncEvent) -> Option<DirtyArea> {
        match event {
            SyncEvent::ApprovalUrl(url) => {
                if self.approval_url.as_deref() == Some(url.as_str()) {
                    return None;
                }
                self.approval_url = Some(url);
                self.set_debug_feedback("Scan to authorize synchronization");
                Some(DirtyArea::Full)
            }
            SyncEvent::Finished(Ok(outcome)) => {
                let had_approval_qr = self.approval_url.is_some();
                self.sync_active = false;
                self.approval_url = None;
                self.last_sync_failure = None;
                match outcome {
                    sync::SyncOutcome::Updated { .. } => {
                        self.bundle_ready = true;
                        self.pending_handoff = Some(BundleHandoff::Updated);
                        self.set_debug_feedback("Synchronization complete");
                        Some(DirtyArea::Full)
                    }
                    sync::SyncOutcome::Cleared { .. } => {
                        self.bundle_ready = true;
                        self.pending_handoff = Some(BundleHandoff::Cleared);
                        self.set_debug_feedback("Library cleared");
                        Some(DirtyArea::Full)
                    }
                    sync::SyncOutcome::Unchanged { .. } => {
                        self.set_debug_feedback("Already current");
                        Some(if had_approval_qr {
                            DirtyArea::Full
                        } else {
                            self.sync_status_dirty()
                        })
                    }
                }
            }
            SyncEvent::Finished(Err(error)) => {
                let had_approval_qr = self.approval_url.is_some();
                self.sync_active = false;
                self.approval_url = None;
                self.last_sync_failure = Some(error.clone());
                self.set_error_feedback("Synchronization failed");
                eprintln!("standalone-test: synchronization failed: {error}");
                Some(if had_approval_qr {
                    DirtyArea::Full
                } else {
                    DirtyArea::StatusFeedback
                })
            }
        }
    }

    fn should_enter_inactivity_sleep(&self, inactivity_timeout: Duration) -> bool {
        self.mode == "ACTIVE"
            && !self.sync_active
            && self.last_activity.elapsed() >= inactivity_timeout
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
        let feedback_damage = if source == InputSourceKind::Touch || event.event_type == EVENT_KEY {
            let was_visible = self.feedback_is_visible().is_some();
            self.clear_feedback();
            was_visible.then_some(DirtyArea::Feedback)
        } else {
            None
        };
        let finish = |dirty: Option<DirtyArea>, action: PowerAction| {
            (merge_optional_dirty(feedback_damage, dirty), action)
        };
        if source == InputSourceKind::Touch {
            self.last_touch = Some(event);
            if event.event_type == EVENT_ABS {
                match event.code {
                    ABS_MT_POSITION_X => {
                        self.touch_raw_x = event.value;
                        self.touch_seen = true;
                        let point = self
                            .orientation
                            .map_multitouch_point(self.touch_raw_x, self.touch_raw_y);
                        self.touch_x = point.x;
                        self.touch_y = point.y;
                    }
                    ABS_MT_POSITION_Y => {
                        self.touch_raw_y = event.value;
                        self.touch_seen = true;
                        let point = self
                            .orientation
                            .map_multitouch_point(self.touch_raw_x, self.touch_raw_y);
                        self.touch_x = point.x;
                        self.touch_y = point.y;
                    }
                    // The T1's legacy compatibility axes are physically
                    // oriented 800x600. Normalize them with the same
                    // orientation that configures the framebuffer.
                    ABS_X => {
                        self.touch_raw_x = event.value;
                        self.touch_seen = true;
                        let point = self
                            .orientation
                            .map_touch_point(self.touch_raw_x, self.touch_raw_y);
                        self.touch_x = point.x;
                        self.touch_y = point.y;
                    }
                    ABS_Y => {
                        self.touch_raw_y = event.value;
                        self.touch_seen = true;
                        let point = self
                            .orientation
                            .map_touch_point(self.touch_raw_x, self.touch_raw_y);
                        self.touch_x = point.x;
                        self.touch_y = point.y;
                    }
                    _ => {}
                }
            }
            if self.touch_down {
                if let Some(action) = self.touch_action {
                    let next_pressed = (display::details_action_at_for_page(
                        self.details_page(),
                        self.touch_x,
                        self.touch_y,
                        self.screen_width(),
                        self.screen_height(),
                    ) == Some(action))
                    .then_some(action);
                    if next_pressed != self.pressed_action {
                        self.pressed_action = next_pressed;
                        return finish(Some(DirtyArea::Action(action)), PowerAction::None);
                    }
                }
            }
            if event.event_type == EVENT_KEY && event.code == BTN_TOUCH {
                if event.value != 0 {
                    if !self.touch_down {
                        self.touch_down = true;
                        return finish(self.begin_touch_action(), PowerAction::None);
                    }
                } else if self.touch_down {
                    self.touch_down = false;
                    let (action, dirty) = self.activate_tap();
                    eprintln!(
                        "standalone-test: touch tap x={} y={} page={:?} action={action:?}",
                        self.touch_x, self.touch_y, self.page
                    );
                    return finish(dirty, action);
                }
            }
            if event.event_type == EVENT_ABS && event.code == ABS_MT_TOUCH_MAJOR {
                if event.value > 0 {
                    if !self.touch_down {
                        self.touch_down = true;
                        return finish(self.begin_touch_action(), PowerAction::None);
                    }
                    self.touch_release_pending = false;
                } else if self.touch_down {
                    self.touch_release_pending = true;
                }
            }
            if event.event_type == EVENT_ABS && event.code == ABS_MT_TRACKING_ID {
                if event.value >= 0 {
                    if !self.touch_down {
                        self.touch_down = true;
                        return finish(self.begin_touch_action(), PowerAction::None);
                    }
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
                    let (action, dirty) = self.activate_tap();
                    eprintln!(
                        "standalone-test: touch tap x={} y={} page={:?} action={action:?}",
                        self.touch_x, self.touch_y, self.page
                    );
                    return finish(dirty, action);
                }
                let dirty = self.is_settings_page().then_some(DirtyArea::Touch);
                return finish(dirty, PowerAction::None);
            }
            return finish(None, PowerAction::None);
        }

        if event.event_type == EVENT_KEY {
            self.last_key = Some((source, event));
            self.key_events += 1;
            if source == InputSourceKind::Keys && event.code == KEY_MENU {
                let (dirty, action) = self.observe_menu(event);
                return finish(dirty, action);
            }
            if source == InputSourceKind::Keys {
                if matches!(event.code, KEY_LEFT | KEY_RIGHT) {
                    if self.page == UiPage::Home && event.value == 1 {
                        let operation = if event.code == KEY_LEFT {
                            ReaderOperation::PreviousPage
                        } else {
                            ReaderOperation::NextPage
                        };
                        self.reader_operation = Some(operation);
                        self.set_debug_feedback(match operation {
                            ReaderOperation::PreviousPage => "Previous page",
                            ReaderOperation::NextPage => "Next page",
                            ReaderOperation::Back
                            | ReaderOperation::ReturnToEntryPoint
                            | ReaderOperation::SetFullscreen(_)
                            | ReaderOperation::SetFontScale(_)
                            | ReaderOperation::DecreaseFontScale
                            | ReaderOperation::ResetFontScale
                            | ReaderOperation::IncreaseFontScale => {
                                unreachable!()
                            }
                        });
                    }
                    // Page buttons have no UI action outside the reader and
                    // must not create a diagnostic-only e-ink update.
                    return finish(None, PowerAction::None);
                }
                if event.code == KEY_HOME {
                    return finish(
                        if event.value == 1 {
                            self.home_button()
                        } else {
                            None
                        },
                        PowerAction::None,
                    );
                }
                if event.code == KEY_BACK {
                    return finish(
                        if event.value == 1 {
                            self.back_button()
                        } else {
                            None
                        },
                        PowerAction::None,
                    );
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
                let (dirty, action) = self.observe_power(source, event);
                return finish(dirty, action);
            }
            let area = if source.is_power() {
                DirtyArea::Power
            } else {
                DirtyArea::Key
            };
            return finish(self.is_settings_page().then_some(area), PowerAction::None);
        }

        finish(None, PowerAction::None)
    }

    fn begin_touch_action(&mut self) -> Option<DirtyArea> {
        self.touch_action_initialized = true;
        self.touch_action = if self.is_settings_page() {
            display::details_action_at_for_page(
                self.details_page(),
                self.touch_x,
                self.touch_y,
                self.screen_width(),
                self.screen_height(),
            )
        } else {
            None
        };
        self.pressed_action = self.touch_action;
        self.touch_action.map(DirtyArea::Action)
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
                    self.set_debug_feedback("Menu held");
                }
                (None, PowerAction::None)
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
                    self.set_debug_feedback("Menu repeat");
                    (None, PowerAction::None)
                }
            }
            0 => {
                let Some(start) = self.menu_press_us.take() else {
                    self.menu_pressed_at = None;
                    self.menu_hold_triggered = false;
                    self.set_debug_feedback("Menu released");
                    return (None, PowerAction::None);
                };
                let duration = event.timestamp_micros().saturating_sub(start);
                self.menu_pressed_at = None;
                let already_triggered = self.menu_hold_triggered;
                self.menu_hold_triggered = false;
                if !already_triggered && duration >= MENU_HOLD_MICROS {
                    (Some(self.trigger_menu_redraw()), PowerAction::None)
                } else if !already_triggered && self.page == UiPage::Home {
                    self.open_ui_page(UiPage::Details, "Details open");
                    (Some(DirtyArea::Full), PowerAction::None)
                } else {
                    self.set_debug_feedback("Menu released");
                    (None, PowerAction::None)
                }
            }
            _ => (None, PowerAction::None),
        }
    }

    fn diagnostics_dirty(&self, area: DirtyArea) -> Option<DirtyArea> {
        self.is_settings_page().then_some(area)
    }

    fn trigger_menu_redraw(&mut self) -> DirtyArea {
        self.menu_hold_triggered = true;
        self.set_debug_feedback("Menu hold - full redraw");
        eprintln!(
            "standalone-test: menu hold reached {}ms; requesting full {} redraw",
            MENU_HOLD_MICROS / 1_000,
            WaveformMode::Gc16.label()
        );
        DirtyArea::Full
    }

    fn activate_tap(&mut self) -> (PowerAction, Option<DirtyArea>) {
        let touch_action = self.touch_action.take().or_else(|| {
            if !self.touch_action_initialized && self.is_settings_page() {
                display::details_action_at_for_page(
                    self.details_page(),
                    self.touch_x,
                    self.touch_y,
                    self.screen_width(),
                    self.screen_height(),
                )
            } else {
                None
            }
        });
        self.touch_action_initialized = false;
        let pressed_action = self.pressed_action.take();
        let pressed_action = pressed_action.or(touch_action);
        if self.page == UiPage::DisplayTest {
            return (PowerAction::None, Some(DirtyArea::Full));
        }
        if self.touch_y < display::STATUS_BAR_HEIGHT as i32 {
            if self.page == UiPage::Home {
                self.open_ui_page(UiPage::Details, "Details open");
            } else {
                self.return_to_reader("Returned to reading");
            }
            return (PowerAction::None, Some(DirtyArea::Full));
        }
        if !self.is_settings_page() {
            self.reader_tap = Some(Point::new(self.touch_x, self.touch_y));
            return (PowerAction::None, None);
        }
        if self.details_page() != display::DetailsPage::Legacy {
            let Some(action) = touch_action else {
                return (PowerAction::None, None);
            };
            if pressed_action != Some(action)
                || display::details_action_at_for_page(
                    self.details_page(),
                    self.touch_x,
                    self.touch_y,
                    self.screen_width(),
                    self.screen_height(),
                ) != Some(action)
            {
                return (PowerAction::None, Some(DirtyArea::Action(action)));
            }
            return self.activate_modern_action(action);
        }
        // An action is committed only if release remains inside the exact
        // control that was pressed. A drag across another control therefore
        // restores the original button without activating either control.
        if touch_action.is_none() && pressed_action.is_none() {
            match display::details_toggle_at(
                self.touch_x,
                self.touch_y,
                self.screen_width(),
                self.screen_height(),
            ) {
                Some(display::DetailsToggle::DebugMessages) => {
                    self.debug_messages = !self.debug_messages;
                    return (PowerAction::None, Some(DirtyArea::Interaction));
                }
                Some(display::DetailsToggle::ReadingProgress) => {
                    self.reading_progress = !self.reading_progress;
                    return (PowerAction::None, Some(DirtyArea::Interaction));
                }
                Some(display::DetailsToggle::FullscreenReader) => {
                    self.reader_operation = Some(ReaderOperation::SetFullscreen(!self.fullscreen));
                    self.set_debug_feedback(if self.fullscreen {
                        "Fullscreen reader off"
                    } else {
                        "Fullscreen reader on"
                    });
                    return (PowerAction::None, None);
                }
                None => {
                    match display::details_preference_at(
                        self.touch_x,
                        self.touch_y,
                        self.screen_width(),
                        self.screen_height(),
                    ) {
                        Some(display::DetailsPreference::FontSize) => {
                            let target = next_font_scale_percent(self.font_scale_percent);
                            self.reader_operation = Some(ReaderOperation::SetFontScale(target));
                            self.set_debug_feedback(format!("Font size {target}% selected"));
                            return (PowerAction::None, Some(DirtyArea::Full));
                        }
                        Some(display::DetailsPreference::Orientation) => {
                            let target = self.orientation.toggle();
                            self.orientation_change = Some(target);
                            self.set_debug_feedback(format!(
                                "Orientation {} selected",
                                target.label()
                            ));
                            return (PowerAction::None, Some(DirtyArea::Full));
                        }
                        Some(
                            display::DetailsPreference::DebugMessages
                            | display::DetailsPreference::ReadingProgress,
                        )
                        | None => {}
                    }
                }
            }
        }
        let Some(touch_action) = touch_action else {
            return (PowerAction::None, None);
        };
        if pressed_action != Some(touch_action)
            || display::details_action_at_for_page(
                self.details_page(),
                self.touch_x,
                self.touch_y,
                self.screen_width(),
                self.screen_height(),
            ) != Some(touch_action)
        {
            return (PowerAction::None, Some(DirtyArea::Action(touch_action)));
        }

        match touch_action {
            display::DetailsAction::SyncNow => {
                self.request_manual_sync();
                (PowerAction::None, Some(DirtyArea::Action(touch_action)))
            }
            display::DetailsAction::ReturnToEntryPoint => {
                self.reader_operation = Some(ReaderOperation::ReturnToEntryPoint);
                self.set_debug_feedback("Returning to entry point");
                (PowerAction::None, Some(DirtyArea::Action(touch_action)))
            }
            display::DetailsAction::DisplayTest => {
                self.open_ui_page(UiPage::DisplayTest, "Display test open");
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::Reboot => {
                self.set_debug_feedback("Reboot requested");
                (PowerAction::Reboot, Some(DirtyArea::Action(touch_action)))
            }
            display::DetailsAction::PowerOff => {
                self.set_debug_feedback("Power off requested");
                (PowerAction::PowerOff, Some(DirtyArea::Action(touch_action)))
            }
            display::DetailsAction::BackToReading => {
                self.return_to_reader("Returned to reading");
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::OpenReading
            | display::DetailsAction::OpenSynchronization
            | display::DetailsAction::OpenDeviceDiagnostics
            | display::DetailsAction::Orientation
            | display::DetailsAction::ShowStatusBar
            | display::DetailsAction::FontDecrease
            | display::DetailsAction::FontReset
            | display::DetailsAction::FontIncrease
            | display::DetailsAction::ReadingProgress
            | display::DetailsAction::DebugMessages => unreachable!(),
        }
    }

    fn activate_modern_action(
        &mut self,
        action: display::DetailsAction,
    ) -> (PowerAction, Option<DirtyArea>) {
        match action {
            display::DetailsAction::OpenReading => {
                self.open_ui_page(UiPage::Reading, "Reading settings open");
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::OpenSynchronization => {
                self.open_ui_page(UiPage::Synchronization, "Synchronization open");
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::OpenDeviceDiagnostics => {
                self.open_ui_page(UiPage::DeviceDiagnostics, "Device settings open");
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::BackToReading => {
                self.return_to_reader("Returned to reading");
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::Orientation => {
                let target = self.orientation.toggle();
                self.orientation_change = Some(target);
                self.set_debug_feedback(format!("Orientation {} selected", target.label()));
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::ShowStatusBar => {
                let fullscreen = !self.fullscreen;
                self.reader_operation = Some(ReaderOperation::SetFullscreen(fullscreen));
                self.set_debug_feedback(if fullscreen {
                    "Status bar hidden"
                } else {
                    "Status bar shown"
                });
                (PowerAction::None, None)
            }
            display::DetailsAction::FontDecrease => {
                self.reader_operation = Some(ReaderOperation::DecreaseFontScale);
                self.set_debug_feedback("Font size decreased");
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::FontReset => {
                self.reader_operation = Some(ReaderOperation::ResetFontScale);
                self.set_debug_feedback("Font size reset");
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::FontIncrease => {
                self.reader_operation = Some(ReaderOperation::IncreaseFontScale);
                self.set_debug_feedback("Font size increased");
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::ReadingProgress => {
                self.reading_progress = !self.reading_progress;
                self.set_debug_feedback(if self.reading_progress {
                    "Reading progress on"
                } else {
                    "Reading progress off"
                });
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::DebugMessages => {
                self.debug_messages = !self.debug_messages;
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::SyncNow => {
                self.request_manual_sync();
                (PowerAction::None, Some(DirtyArea::Action(action)))
            }
            display::DetailsAction::ReturnToEntryPoint => {
                self.reader_operation = Some(ReaderOperation::ReturnToEntryPoint);
                self.set_debug_feedback("Returning to entry point");
                (PowerAction::None, Some(DirtyArea::Action(action)))
            }
            display::DetailsAction::DisplayTest => {
                self.open_ui_page(UiPage::DisplayTest, "Display test open");
                (PowerAction::None, Some(DirtyArea::Full))
            }
            display::DetailsAction::Reboot => {
                self.set_debug_feedback("Reboot requested");
                (PowerAction::Reboot, Some(DirtyArea::Action(action)))
            }
            display::DetailsAction::PowerOff => {
                self.set_debug_feedback("Power off requested");
                (PowerAction::PowerOff, Some(DirtyArea::Action(action)))
            }
        }
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
                self.set_debug_feedback("Wake power ignored");
                return (self.diagnostics_dirty(DirtyArea::Power), PowerAction::None);
            }
            self.ignore_power_until = None;
        }
        match event.value {
            1 => {
                if self.power_press_us.is_none() {
                    self.power_press_us = Some(event.timestamp_micros());
                }
                self.set_debug_feedback("Power held");
                (self.diagnostics_dirty(DirtyArea::Power), PowerAction::None)
            }
            0 => {
                let duration = self
                    .power_press_us
                    .take()
                    .map(|start| event.timestamp_micros().saturating_sub(start))
                    .unwrap_or_default();
                self.last_power_duration_ms = Some(duration / 1_000);
                if duration >= LONG_PRESS_MICROS {
                    self.set_debug_feedback("Long power - reboot");
                    eprintln!(
                        "standalone-test: power release source={} duration_ms={} action=REBOOT",
                        source.label(),
                        duration / 1_000
                    );
                    (
                        self.diagnostics_dirty(DirtyArea::Power),
                        PowerAction::Reboot,
                    )
                } else {
                    self.set_debug_feedback("Short power - sleep");
                    eprintln!(
                        "standalone-test: power release source={} duration_ms={} action=SLEEP",
                        source.label(),
                        duration / 1_000
                    );
                    (self.diagnostics_dirty(DirtyArea::Power), PowerAction::Sleep)
                }
            }
            2 => {
                self.set_debug_feedback("Power repeat");
                (self.diagnostics_dirty(DirtyArea::Power), PowerAction::None)
            }
            _ => (self.diagnostics_dirty(DirtyArea::Power), PowerAction::None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        display, record_reader_event_feedback, BundleHandoff, DirtyArea, Feedback, InputSourceKind,
        PageTone, Point, PowerAction, ReaderOperation, ReaderOrientation, ReaderPreferences,
        RefreshReason, SuspendMode, SyncEvent, SyncTrigger, UiPage, UiState, ABS_MT_POSITION_X,
        ABS_MT_POSITION_Y, ABS_MT_TOUCH_MAJOR, ABS_MT_TRACKING_ID, ABS_X, ABS_Y, BTN_TOUCH,
        EVENT_ABS, EVENT_KEY, EVENT_SYN, KEY_BACK, KEY_HOME, KEY_LEFT, KEY_MENU, KEY_RIGHT,
        SYN_REPORT,
    };
    use crate::input::RawEvent;
    use prs_markdown::reader::ReaderEvent;
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
    fn settings_menu_opens_each_nested_section_and_back_walks_to_reader() {
        let mut state = UiState::new();
        state.open_ui_page(UiPage::Details, "Settings open");
        assert!(state.settings_menu);

        let region = display::details_action_region_for_page(
            display::DetailsPage::Menu,
            display::DetailsAction::OpenReading,
            600,
            800,
        )
        .expect("reading section region");
        state.touch_x = region.left as i32 + 4;
        state.touch_y = region.top as i32 + 4;
        state.touch_down = true;
        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));
        assert_eq!(state.page, UiPage::Reading);

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_BACK, 1, 1_000_001));
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.page, UiPage::Details);
        assert!(state.settings_menu);

        state.observe(InputSourceKind::Keys, event(KEY_BACK, 1, 1_000_002));
        assert_eq!(state.page, UiPage::Home);
        assert!(state.ui_history.is_empty());
    }

    #[test]
    fn modern_reading_controls_dispatch_orientation_status_font_and_progress() {
        let mut state = UiState::new();
        state.page = UiPage::Reading;

        let tap = |state: &mut UiState, action: display::DetailsAction| {
            let region = display::details_action_region_for_page(
                display::DetailsPage::Reading,
                action,
                600,
                800,
            )
            .expect("reading control region");
            state.touch_x = region.left as i32 + 4;
            state.touch_y = region.top as i32 + 4;
            state.touch_down = true;
            state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));
        };

        tap(&mut state, display::DetailsAction::Orientation);
        assert_eq!(
            state.take_orientation_change(),
            Some(ReaderOrientation::Landscape)
        );
        tap(&mut state, display::DetailsAction::ShowStatusBar);
        assert_eq!(
            state.take_reader_operation(),
            Some(ReaderOperation::SetFullscreen(true))
        );
        for (action, expected) in [
            (
                display::DetailsAction::FontDecrease,
                ReaderOperation::DecreaseFontScale,
            ),
            (
                display::DetailsAction::FontReset,
                ReaderOperation::ResetFontScale,
            ),
            (
                display::DetailsAction::FontIncrease,
                ReaderOperation::IncreaseFontScale,
            ),
        ] {
            tap(&mut state, action);
            assert_eq!(state.take_reader_operation(), Some(expected));
        }
        assert!(state.reading_progress);
        tap(&mut state, display::DetailsAction::ReadingProgress);
        assert!(!state.reading_progress);
    }

    #[test]
    fn modern_device_and_sync_controls_are_reachable_in_landscape() {
        let mut state = UiState::with_orientation(ReaderOrientation::Landscape);
        state.page = UiPage::DeviceDiagnostics;
        let debug = display::details_action_region_for_page(
            display::DetailsPage::DeviceDiagnostics,
            display::DetailsAction::DebugMessages,
            800,
            600,
        )
        .expect("debug control region");
        state.touch_x = debug.left as i32 + 4;
        state.touch_y = debug.top as i32 + 4;
        state.touch_down = true;
        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));
        assert!(state.debug_messages);

        let mut state = UiState::with_orientation(ReaderOrientation::Landscape);
        state.page = UiPage::Synchronization;
        let sync = display::details_action_region_for_page(
            display::DetailsPage::Synchronization,
            display::DetailsAction::SyncNow,
            800,
            600,
        )
        .expect("sync control region");
        state.touch_x = sync.left as i32 + 4;
        state.touch_y = sync.top as i32 + 4;
        state.touch_down = true;
        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));
        assert_eq!(state.take_sync_trigger(), Some(SyncTrigger::Manual));
    }

    #[test]
    fn modern_settings_press_is_drawn_before_release() {
        let mut state = UiState::new();
        state.open_ui_page(UiPage::Details, "Settings open");
        let region = display::details_action_region_for_page(
            display::DetailsPage::Menu,
            display::DetailsAction::OpenDeviceDiagnostics,
            600,
            800,
        )
        .expect("device section region");
        state.touch_x = region.left as i32 + 4;
        state.touch_y = region.top as i32 + 4;
        let (dirty, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 1, 1_000_000));
        assert_eq!(
            dirty,
            Some(DirtyArea::Action(
                display::DetailsAction::OpenDeviceDiagnostics
            ))
        );
        assert_eq!(action, PowerAction::None);
        assert_eq!(
            state.pressed_action,
            Some(display::DetailsAction::OpenDeviceDiagnostics)
        );
    }

    #[test]
    fn content_tap_is_deferred_to_the_markdown_reader() {
        let mut state = UiState::new();
        state.touch_x = 500;
        state.touch_y = display::CONTENT_TOP as i32 + 80;
        state.touch_down = true;

        let (dirty, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));

        assert_eq!(action, super::PowerAction::None);
        assert_eq!(dirty, None);
        assert_eq!(state.take_reader_tap(), Some(Point::new(500, 156)));
        assert_eq!(state.page, UiPage::Home);
    }

    #[test]
    fn external_link_overlay_uses_bounded_transient_damage() {
        assert_eq!(
            DirtyArea::ExternalLinkOverlay.reason(UiPage::Home),
            RefreshReason::Transient
        );
        assert_eq!(
            DirtyArea::ExternalLinkOverlay.region(600, 800, UiPage::Home),
            super::reader::external_link_overlay_region(600, 800)
        );
        assert_eq!(
            super::reader::external_link_overlay_region(600, 800),
            super::DisplayRegion::new(228, 570, 360, 214)
        );
    }

    #[test]
    fn external_url_events_route_to_overlay_without_reader_navigation() {
        let event = ReaderEvent::ExternalUrl("https://example.com/reader".into());
        assert_eq!(
            super::reader_event_dirty_area(&event, PageTone::Monochrome),
            Some(DirtyArea::ExternalLinkOverlay)
        );
        assert_eq!(
            super::reader_event_message(&event),
            "External link: https://example.com/reader"
        );
        assert_eq!(
            super::reader_event_dirty_area(&ReaderEvent::NoAction, PageTone::Monochrome),
            None
        );
    }

    #[test]
    fn next_touch_or_physical_button_event_dismisses_external_overlay_first() {
        let touch = RawEvent {
            event_type: EVENT_ABS,
            code: ABS_MT_POSITION_X,
            value: 400,
            ..event(0, 0, 1_000_000)
        };
        let button = event(KEY_RIGHT, 1, 1_000_001);
        let non_interaction = RawEvent {
            event_type: EVENT_ABS,
            code: ABS_MT_POSITION_Y,
            value: 600,
            ..event(0, 0, 1_000_002)
        };

        assert!(super::dismisses_external_link_overlay(
            InputSourceKind::Touch,
            touch
        ));
        assert!(super::dismisses_external_link_overlay(
            InputSourceKind::Keys,
            button
        ));
        assert!(!super::dismisses_external_link_overlay(
            InputSourceKind::Keys,
            non_interaction
        ));
    }

    #[test]
    fn active_mode_is_hidden_from_status_bar() {
        let state = UiState::new();
        let lines = super::screen_lines(&state, true);
        assert_eq!(lines[0].split('|').nth(4), Some(""));
    }

    #[test]
    fn syncing_uses_status_bar_mode_and_finishes_quietly_or_with_an_error() {
        let mut state = UiState::new();
        state.sync_started();
        assert_eq!(
            super::screen_view_model(&state, true).status_bar.mode,
            "SYNCING"
        );

        state.apply_sync_event(SyncEvent::Finished(Ok(
            super::sync::SyncOutcome::Unchanged {
                revision: prs_sync_protocol::InboxRevision::new(3),
            },
        )));
        assert!(!state.sync_active);
        assert_eq!(super::screen_view_model(&state, true).status_bar.mode, "");
        assert_eq!(state.visible_feedback(), None);

        state.sync_started();
        state.apply_sync_event(SyncEvent::Finished(Err("network loss".into())));
        assert!(!state.sync_active);
        assert_eq!(state.visible_feedback(), Some("Synchronization failed"));
        assert_eq!(super::screen_view_model(&state, true).status_bar.mode, "");
    }

    #[test]
    fn unchanged_authorization_poll_does_not_request_a_redraw() {
        let mut state = UiState::new();
        state.sync_started();

        assert_eq!(
            state.apply_sync_event(SyncEvent::ApprovalUrl(
                "https://reader.example.test/a/request".into(),
            )),
            Some(DirtyArea::Full)
        );
        assert_eq!(
            state.apply_sync_event(SyncEvent::ApprovalUrl(
                "https://reader.example.test/a/request".into(),
            )),
            None
        );
    }

    #[test]
    fn sync_transitions_use_bounded_status_damage_without_a_qr() {
        let mut state = UiState::new();
        assert_eq!(
            super::merge_optional_dirty(None, Some(state.sync_status_dirty())),
            Some(DirtyArea::SyncStatus)
        );
        assert_eq!(
            DirtyArea::SyncStatus.region(600, 800, UiPage::Home),
            super::DisplayRegion::new(0, 0, 600, display::STATUS_BAR_HEIGHT as u32)
        );

        state.sync_started();
        assert_eq!(
            state.apply_sync_event(SyncEvent::Finished(Ok(
                super::sync::SyncOutcome::Unchanged {
                    revision: prs_sync_protocol::InboxRevision::new(11),
                },
            ))),
            Some(DirtyArea::SyncStatus)
        );
    }

    #[test]
    fn immediate_failed_sync_merges_bounded_status_feedback_damage() {
        let mut state = UiState::new();
        let mut redraw_area = None;

        state.request_manual_sync();
        assert!(super::start_requested_sync(&mut state, || true).is_some());
        redraw_area = super::merge_optional_dirty(redraw_area, Some(state.sync_status_dirty()));
        redraw_area = super::merge_optional_dirty(
            redraw_area,
            state.apply_sync_event(SyncEvent::Finished(Err("network loss".into()))),
        );

        assert_eq!(redraw_area, Some(DirtyArea::StatusFeedback));
        assert_eq!(
            DirtyArea::StatusFeedback.region(600, 800, UiPage::Home),
            super::DisplayRegion::new(0, 0, 600, display::CONTENT_TOP as u32)
        );
    }

    #[test]
    fn inert_details_tap_requests_no_display_update() {
        let mut state = UiState::new();
        state.page = UiPage::Details;
        state.touch_x = 100;
        state.touch_y = 250;
        state.touch_down = true;

        let (dirty, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));

        assert_eq!(dirty, None);
        assert_eq!(action, PowerAction::None);
    }

    #[test]
    fn only_visible_feedback_clear_requests_the_feedback_strip() {
        let mut state = UiState::new();
        state.set_error_feedback("Reader error");
        let (dirty, action) = state.observe(InputSourceKind::Keys, event(999, 0, 1_000_000));
        assert_eq!(dirty, Some(DirtyArea::Feedback));
        assert_eq!(action, PowerAction::None);
        assert_eq!(
            DirtyArea::Feedback.region(600, 800, UiPage::Home),
            super::DisplayRegion::new(
                0,
                display::STATUS_BAR_HEIGHT as u32,
                600,
                (display::CONTENT_TOP - display::STATUS_BAR_HEIGHT) as u32,
            )
        );

        let mut state = UiState::new();
        let (dirty, action) = state.observe(InputSourceKind::Keys, event(999, 0, 1_000_000));
        assert_eq!(dirty, None);
        assert_eq!(action, PowerAction::None);
    }

    #[test]
    fn routine_reader_events_are_hidden_unless_debug_messages_are_enabled() {
        let mut state = UiState::new();
        let event = ReaderEvent::PageChanged {
            page: 1,
            page_count: 4,
        };
        record_reader_event_feedback(&mut state, &event);
        assert_eq!(state.visible_feedback(), None);

        state.debug_messages = true;
        assert_eq!(state.visible_feedback(), Some("Page 2 of 4"));
    }

    #[test]
    fn interaction_clears_feedback_before_processing_the_next_event() {
        let mut state = UiState::new();
        state.set_error_feedback("Reader error");
        let (_, action) = state.observe(InputSourceKind::Keys, event(999, 0, 1_000_000));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.visible_feedback(), None);

        state.set_error_feedback("Reader error");
        state.touch_x = 520;
        state.touch_y = display::CONTENT_TOP as i32 + 20;
        state.touch_down = true;
        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_001));
        assert_eq!(state.visible_feedback(), None);
    }

    #[test]
    fn home_button_feedback_clear_is_in_the_visible_redraw_region() {
        let mut state = UiState::new();
        state.set_error_feedback("Reader error");

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_HOME, 1, 1_000_000));

        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.visible_feedback(), None);
        let redraw = dirty
            .expect("physical button interaction should request a redraw")
            .region(600, 800, UiPage::Home);
        let feedback = super::DisplayRegion::new(
            0,
            display::STATUS_BAR_HEIGHT as u32,
            600,
            (display::CONTENT_TOP - display::STATUS_BAR_HEIGHT) as u32,
        );
        assert!(redraw.contains(feedback));
    }

    #[test]
    fn debug_messages_toggle_is_off_by_default_and_tappable_in_details() {
        let mut state = UiState::new();
        assert!(!state.debug_messages);
        state.page = UiPage::Details;
        state.touch_x = 100;
        state.touch_y = (display::DETAILS_DEBUG_TOP + 10) as i32;
        state.touch_down = true;

        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));
        assert!(state.debug_messages);

        state.touch_down = true;
        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_001));
        assert!(!state.debug_messages);
    }

    #[test]
    fn reading_progress_toggle_is_on_by_default_and_tappable_in_details() {
        let mut state = UiState::new();
        assert!(state.reading_progress);
        state.page = UiPage::Details;
        state.touch_x = 100;
        state.touch_y = (display::DETAILS_PROGRESS_TOP + 10) as i32;
        state.touch_down = true;

        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));
        assert!(!state.reading_progress);

        state.touch_down = true;
        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_001));
        assert!(state.reading_progress);
    }

    #[test]
    fn fullscreen_reader_toggle_is_off_by_default_and_requests_layout_reflow() {
        let mut state = UiState::new();
        assert!(!state.fullscreen);
        state.page = UiPage::Details;
        state.touch_x = 100;
        state.touch_y = (display::DETAILS_FULLSCREEN_TOP + 10) as i32;
        state.touch_down = true;

        let (dirty, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));

        assert_eq!(action, PowerAction::None);
        assert_eq!(dirty, None);
        assert_eq!(
            state.take_reader_operation(),
            Some(ReaderOperation::SetFullscreen(true))
        );
        assert!(!state.fullscreen);

        state.fullscreen = true;
        state.touch_down = true;
        let (_, _) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_001));
        assert_eq!(
            state.take_reader_operation(),
            Some(ReaderOperation::SetFullscreen(false))
        );
    }

    #[test]
    fn orientation_preference_is_selectable_without_changing_persistence_state() {
        let mut state = UiState::new();
        state.page = UiPage::Details;
        state.touch_x = 100;
        state.touch_y = (display::DETAILS_ORIENTATION_TOP + 10) as i32;
        state.touch_down = true;

        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));

        assert_eq!(state.orientation, ReaderOrientation::Portrait);
        assert_eq!(
            state.take_orientation_change(),
            Some(ReaderOrientation::Landscape)
        );
    }

    #[test]
    fn font_size_preference_cycles_and_requests_reader_reflow() {
        let mut state = UiState::new();
        assert_eq!(state.font_scale_percent, 100);
        state.page = UiPage::Details;
        state.touch_x = 100;
        state.touch_y = (display::DETAILS_FONT_SIZE_TOP + 10) as i32;
        state.touch_down = true;

        let (dirty, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_000));

        assert_eq!(action, PowerAction::None);
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(
            state.take_reader_operation(),
            Some(ReaderOperation::SetFontScale(125))
        );
    }

    #[test]
    fn saved_preferences_seed_orientation_and_font_size_state() {
        let state = UiState::with_preferences(ReaderPreferences {
            orientation: ReaderOrientation::Landscape,
            font_scale_percent: 150,
        });

        assert_eq!(state.orientation, ReaderOrientation::Landscape);
        assert_eq!(state.font_scale_percent, 150);
    }

    #[test]
    fn legacy_touch_axes_follow_landscape_framebuffer_orientation() {
        let mut state = UiState::with_orientation(ReaderOrientation::Landscape);
        let mut axis_x = event(ABS_X, 770, 1_000_000);
        axis_x.event_type = EVENT_ABS;
        let mut axis_y = event(ABS_Y, 300, 1_000_001);
        axis_y.event_type = EVENT_ABS;

        state.observe(InputSourceKind::Touch, axis_x);
        state.observe(InputSourceKind::Touch, axis_y);

        assert_eq!(state.touch_x, 770);
        assert_eq!(state.touch_y, 300);
    }

    #[test]
    fn multitouch_coordinates_reach_landscape_settings_targets() {
        let mut state = UiState::with_orientation(ReaderOrientation::Landscape);
        state.page = UiPage::Details;

        // The physical multitouch stream reports the portrait-sized pair.
        // The inverse axis order reaches the landscape Orientation row.
        let mut position_y = event(ABS_MT_POSITION_Y, 100, 1_000_000);
        position_y.event_type = EVENT_ABS;
        let mut position_x = event(ABS_MT_POSITION_X, 194, 1_000_001);
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
        assert_eq!(state.touch_x, 100);
        assert_eq!(state.touch_y, 194);
        assert_eq!(
            state.take_orientation_change(),
            Some(ReaderOrientation::Portrait)
        );
    }

    #[test]
    fn cancelled_sync_clears_status_bar_state() {
        let mut state = UiState::new();
        state.sync_started();
        state.sync_cancelled();
        assert!(!state.sync_active);
        assert_eq!(state.visible_feedback(), Some("Synchronization cancelled"));
        assert_eq!(super::screen_view_model(&state, true).status_bar.mode, "");
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
            DirtyArea::SyncStatus.reason(UiPage::Home),
            RefreshReason::StatusBar
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
        assert_eq!(dirty, None);
        assert_eq!(action, super::PowerAction::None);

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_MENU, 0, 2_000_000));
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(action, super::PowerAction::None);
        assert!(state.menu_press_us.is_none());
    }

    #[test]
    fn short_menu_press_opens_details_only_from_reader() {
        let mut state = UiState::new();
        state.observe(InputSourceKind::Keys, event(KEY_MENU, 1, 1_000_000));

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_MENU, 0, 1_999_999));
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.page, UiPage::Details);
        assert_eq!(state.ui_history, vec![UiPage::Home]);
        assert_eq!(state.take_reader_operation(), None);
    }

    #[test]
    fn home_from_reader_queues_entry_point_and_clears_ui_history() {
        let mut state = UiState::new();
        state.ui_history = vec![UiPage::Details];

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_HOME, 1, 1_000_000));

        assert_eq!(dirty, None);
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.page, UiPage::Home);
        assert!(state.ui_history.is_empty());
        assert_eq!(
            state.take_reader_operation(),
            Some(ReaderOperation::ReturnToEntryPoint)
        );
    }

    #[test]
    fn home_from_non_reader_returns_to_reader_and_clears_ui_history() {
        let mut state = UiState::new();
        state.page = UiPage::DisplayTest;
        state.ui_history = vec![UiPage::Home, UiPage::Details];

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_HOME, 1, 1_000_000));

        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.page, UiPage::Home);
        assert!(state.ui_history.is_empty());
        assert_eq!(state.take_reader_operation(), None);
    }

    #[test]
    fn back_walks_ui_history_then_delegates_to_reader_history() {
        let mut state = UiState::new();
        state.open_ui_page(UiPage::Details, "Details open");
        state.open_ui_page(UiPage::DisplayTest, "Display test open");

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_BACK, 1, 1_000_000));
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.page, UiPage::Details);
        assert_eq!(state.ui_history, vec![UiPage::Home]);
        assert_eq!(state.take_reader_operation(), None);

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_BACK, 1, 1_000_001));
        assert_eq!(dirty, Some(DirtyArea::Full));
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.page, UiPage::Home);
        assert!(state.ui_history.is_empty());

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_BACK, 1, 1_000_002));
        assert_eq!(dirty, None);
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.take_reader_operation(), Some(ReaderOperation::Back));
    }

    #[test]
    fn back_from_empty_reader_history_is_a_no_op_without_refresh() {
        let mut state = UiState::new();

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_BACK, 1, 1_000_000));

        assert_eq!(dirty, None);
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.take_reader_operation(), Some(ReaderOperation::Back));
    }

    #[test]
    fn menu_short_press_is_a_no_op_outside_reader() {
        for page in [UiPage::Details, UiPage::DisplayTest] {
            let mut state = UiState::new();
            state.page = page;
            state.ui_history = vec![UiPage::Home];
            state.observe(InputSourceKind::Keys, event(KEY_MENU, 1, 1_000_000));

            let (dirty, action) =
                state.observe(InputSourceKind::Keys, event(KEY_MENU, 0, 1_100_000));

            assert_eq!(dirty, None);
            assert_eq!(action, PowerAction::None);
            assert_eq!(state.page, page);
            assert_eq!(state.ui_history, vec![UiPage::Home]);
            assert_eq!(state.take_reader_operation(), None);
        }
    }

    #[test]
    fn completed_menu_hold_refreshes_without_opening_details() {
        for page in [UiPage::Home, UiPage::Details, UiPage::DisplayTest] {
            let mut state = UiState::new();
            state.page = page;
            state.observe(InputSourceKind::Keys, event(KEY_MENU, 1, 1_000_000));
            state.menu_pressed_at = Some(Instant::now() - Duration::from_secs(1));

            assert_eq!(state.poll_menu_hold(), Some(DirtyArea::Full));
            let (dirty, action) =
                state.observe(InputSourceKind::Keys, event(KEY_MENU, 0, 2_000_000));

            assert_eq!(dirty, None);
            assert_eq!(action, PowerAction::None);
            assert_eq!(state.page, page);
            assert_eq!(state.take_reader_operation(), None);
        }
    }

    #[test]
    fn page_buttons_queue_one_reader_page_operation_on_press() {
        let mut state = UiState::new();

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_LEFT, 1, 1_000_000));
        assert_eq!(dirty, None);
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(
            state.take_reader_operation(),
            Some(ReaderOperation::PreviousPage)
        );

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_LEFT, 2, 1_000_001));
        assert_eq!(dirty, None);
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.take_reader_operation(), None);

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_RIGHT, 1, 1_000_002));
        assert_eq!(dirty, None);
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
        assert_eq!(dirty, None);
        assert_eq!(action, super::PowerAction::None);
        assert_eq!(state.take_reader_operation(), None);

        state.observe(InputSourceKind::Keys, event(KEY_MENU, 1, 2_000_000));
        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_MENU, 0, 2_100_000));
        assert_eq!(dirty, None);
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
    fn details_action_press_is_visible_before_release_activation() {
        let mut state = UiState::new();
        state.page = UiPage::Details;
        state.touch_x = 100;
        state.touch_y = display::DETAILS_SYNC_TOP as i32 + 10;

        let (dirty, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 1, 1_000_000));

        assert_eq!(
            dirty,
            Some(DirtyArea::Action(display::DetailsAction::SyncNow))
        );
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.pressed_action, Some(display::DetailsAction::SyncNow));
        assert!(!state.sync_requested);

        let (dirty, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_100_000));

        assert_eq!(
            dirty,
            Some(DirtyArea::Action(display::DetailsAction::SyncNow))
        );
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.pressed_action, None);
        assert_eq!(state.take_sync_trigger(), Some(super::SyncTrigger::Manual));
    }

    #[test]
    fn details_action_drag_outside_cancels_without_cross_button_activation() {
        let mut state = UiState::new();
        state.page = UiPage::Details;
        state.touch_x = 100;
        state.touch_y = display::DETAILS_SYNC_TOP as i32 + 10;
        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 1, 1_000_000));

        let mut moved = event(
            ABS_MT_POSITION_Y,
            display::DETAILS_POWER_OFF_TOP as i32 + 10,
            1_000_001,
        );
        moved.event_type = EVENT_ABS;
        let (dirty, action) = state.observe(InputSourceKind::Touch, moved);
        assert_eq!(
            dirty,
            Some(DirtyArea::Action(display::DetailsAction::SyncNow))
        );
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.pressed_action, None);

        let (dirty, action) = state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_100_000));
        assert_eq!(
            dirty,
            Some(DirtyArea::Action(display::DetailsAction::SyncNow))
        );
        assert_eq!(action, PowerAction::None);
        assert_eq!(state.page, UiPage::Details);
        assert_eq!(state.take_sync_trigger(), None);
    }

    #[test]
    fn production_details_model_fits_before_the_dedicated_action_pane() {
        let state = UiState::new();
        let view = super::screen_view_model(&state, false);
        let max_rows = (display::DETAILS_ACTION_HEADER_TOP
            .saturating_sub(4 + 16 + display::CONTENT_TOP))
            / display::DETAILS_LINE_STEP;

        assert!(
            view.details.rows.len() <= max_rows,
            "production Details model has {} rows; the action pane allows at most {max_rows}",
            view.details.rows.len()
        );
        assert!(view.details.rows.iter().any(|row| {
            matches!(
                row,
                display::DetailsRow::Value(text) if text.starts_with("SD card ")
            )
        }));
        assert!(view.details.rows.iter().any(|row| {
            matches!(
                row,
                display::DetailsRow::Section(text) if text == "Diagnostics"
            )
        }));

        for (index, _) in view.details.rows.iter().enumerate() {
            let top = display::CONTENT_TOP + (index + 1) * display::DETAILS_LINE_STEP;
            assert!(
                top + 16 <= display::DETAILS_ACTION_HEADER_TOP - 4,
                "production row {index} at y={top} intersects the action pane"
            );
        }
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
        assert_eq!(state.take_sync_trigger(), Some(super::SyncTrigger::Manual));

        state.touch_down = true;
        state.touch_y = super::display::DETAILS_RETURN_ENTRY_TOP as i32 + 10;
        state.observe(InputSourceKind::Touch, event(BTN_TOUCH, 0, 1_000_001));
        assert_eq!(
            state.take_reader_operation(),
            Some(ReaderOperation::ReturnToEntryPoint)
        );
    }

    #[test]
    fn sleep_inactivity_timeout_defaults_to_five_minutes() {
        assert_eq!(
            super::parse_sleep_inactivity_timeout(None).unwrap(),
            Duration::from_secs(5 * 60)
        );
    }

    #[test]
    fn sleep_inactivity_timeout_accepts_a_positive_override() {
        assert_eq!(
            super::parse_sleep_inactivity_timeout(Some("37".into())).unwrap(),
            Duration::from_secs(37)
        );
    }

    #[test]
    fn sleep_inactivity_timeout_rejects_zero_and_non_numeric_values() {
        assert!(super::parse_sleep_inactivity_timeout(Some("0".into())).is_err());
        assert!(super::parse_sleep_inactivity_timeout(Some("later".into())).is_err());
    }

    #[test]
    fn inactivity_enters_sleep_at_the_configured_quiet_period() {
        let mut state = UiState::new();
        let timeout = Duration::from_secs(5 * 60);
        state.last_activity = Instant::now() - timeout - Duration::from_secs(1);

        assert!(state.should_enter_inactivity_sleep(timeout));
        state.enter_sleep(SuspendMode::EInk);
        assert!(!state.should_enter_inactivity_sleep(timeout));
    }

    #[test]
    fn activity_resets_the_inactivity_sleep_timer() {
        let mut state = UiState::new();
        let timeout = Duration::from_secs(5 * 60);
        state.last_activity = Instant::now() - timeout - Duration::from_secs(1);

        state.observe(InputSourceKind::Keys, event(KEY_RIGHT, 2, 1_000_000));

        assert!(!state.should_enter_inactivity_sleep(timeout));
    }

    #[test]
    fn wake_transition_starts_one_automatic_sync_per_episode() {
        let mut state = UiState::new();
        state.enter_sleep(SuspendMode::EInk);
        assert!(state.wake());
        let mut starts = 0;
        assert_eq!(
            super::start_requested_sync(&mut state, || {
                starts += 1;
                true
            }),
            Some(super::SyncTrigger::Wake)
        );
        state.apply_sync_event(SyncEvent::Finished(Ok(
            super::sync::SyncOutcome::Unchanged {
                revision: prs_sync_protocol::InboxRevision::new(7),
            },
        )));
        assert_eq!(
            super::start_requested_sync(&mut state, || {
                starts += 1;
                true
            }),
            None
        );
        assert_eq!(starts, 1);
        assert!(!state.wake());
        assert!(!state.should_enter_inactivity_sleep(Duration::from_secs(5 * 60)));

        state.enter_sleep(SuspendMode::EInk);
        assert!(state.wake());
        assert_eq!(
            super::start_requested_sync(&mut state, || {
                starts += 1;
                true
            }),
            Some(super::SyncTrigger::Wake)
        );
        assert_eq!(starts, 2);
    }

    #[test]
    fn failed_wake_sync_does_not_request_another_sync_while_awake() {
        let mut state = UiState::new();
        state.enter_sleep(SuspendMode::EInk);
        assert!(state.wake());
        let mut starts = 0;
        assert_eq!(
            super::start_requested_sync(&mut state, || {
                starts += 1;
                true
            }),
            Some(super::SyncTrigger::Wake)
        );
        state.apply_sync_event(SyncEvent::Finished(Err("network loss".into())));

        assert!(!state.sync_active);
        assert!(!state.bundle_ready);
        assert_eq!(state.last_sync_failure.as_deref(), Some("network loss"));
        assert_eq!(
            super::start_requested_sync(&mut state, || {
                starts += 1;
                true
            }),
            None
        );
        assert_eq!(starts, 1);
        assert!(!state.should_enter_inactivity_sleep(Duration::from_secs(5 * 60)));
    }

    #[test]
    fn manual_sync_request_remains_available_after_wake_failure() {
        let mut state = UiState::new();
        state.enter_sleep(SuspendMode::EInk);
        assert!(state.wake());
        let mut starts = 0;
        assert_eq!(
            super::start_requested_sync(&mut state, || {
                starts += 1;
                true
            }),
            Some(super::SyncTrigger::Wake)
        );
        state.apply_sync_event(SyncEvent::Finished(Err("network loss".into())));
        state.request_manual_sync();

        assert_eq!(
            super::start_requested_sync(&mut state, || {
                starts += 1;
                true
            }),
            Some(super::SyncTrigger::Manual)
        );
        assert_eq!(starts, 2);
    }

    #[test]
    fn successful_wake_syncs_do_not_restart_while_awake() {
        for outcome in [
            super::sync::SyncOutcome::Updated {
                revision: prs_sync_protocol::InboxRevision::new(8),
                entry_point: "index.md".into(),
            },
            super::sync::SyncOutcome::Cleared {
                revision: prs_sync_protocol::InboxRevision::new(9),
            },
            super::sync::SyncOutcome::Unchanged {
                revision: prs_sync_protocol::InboxRevision::new(10),
            },
        ] {
            let mut state = UiState::new();
            state.enter_sleep(SuspendMode::EInk);
            assert!(state.wake());
            let mut starts = 0;
            assert_eq!(
                super::start_requested_sync(&mut state, || {
                    starts += 1;
                    true
                }),
                Some(super::SyncTrigger::Wake)
            );
            state.apply_sync_event(SyncEvent::Finished(Ok(outcome)));
            assert_eq!(
                super::start_requested_sync(&mut state, || {
                    starts += 1;
                    true
                }),
                None
            );
            assert_eq!(starts, 1);
        }
    }

    #[test]
    fn updated_and_cleared_syncs_both_schedule_reader_handoffs() {
        let mut state = UiState::new();
        state.sync_started();
        state.apply_sync_event(SyncEvent::Finished(Ok(super::sync::SyncOutcome::Cleared {
            revision: prs_sync_protocol::InboxRevision::new(4),
        })));

        assert!(!state.sync_active);
        assert!(state.bundle_ready);
        assert_eq!(state.pending_handoff, Some(BundleHandoff::Cleared));
        assert!(matches!(
            state.feedback,
            Some(Feedback::Debug(ref message)) if message == "Library cleared"
        ));

        state.sync_started();
        state.apply_sync_event(SyncEvent::Finished(Ok(super::sync::SyncOutcome::Updated {
            revision: prs_sync_protocol::InboxRevision::new(5),
            entry_point: "index.md".into(),
        })));
        assert!(state.bundle_ready);
        assert_eq!(state.pending_handoff, Some(BundleHandoff::Updated));
    }

    #[test]
    fn short_menu_press_does_not_navigate_from_display_test() {
        let mut state = UiState::new();
        state.page = UiPage::DisplayTest;
        state.observe(InputSourceKind::Keys, event(KEY_MENU, 1, 1_000_000));

        let (dirty, action) = state.observe(InputSourceKind::Keys, event(KEY_MENU, 0, 1_100_000));

        assert_eq!(action, super::PowerAction::None);
        assert_eq!(dirty, None);
        assert_eq!(state.page, UiPage::DisplayTest);
        assert_eq!(state.take_reader_operation(), None);
    }

    #[test]
    fn legacy_touch_axes_follow_portrait_rotation_and_tracking_release_activates_tap() {
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
