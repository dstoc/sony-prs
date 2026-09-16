use crate::framebuffer::{DisplayRegion, NativeDisplay, WaveformMode};
use crate::input::{EventReader, RawEvent};
use crate::{display, input};
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::raw::c_int;
use std::path::Path;
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
const LONG_PRESS_MICROS: u64 = 2_000_000;
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
    let mut state = UiState::new();
    let mut adb_restart_pending = false;

    eprintln!(
        "standalone-test: framebuffer={}x{}; zygote must already be stopped",
        display.width(),
        display.height()
    );
    redraw(&mut display, &state, wake_lock.is_held(), DirtyArea::Full)
        .map_err(|error| display_error("initial redraw", error))?;

    loop {
        if adb_restart_pending && usb_power_online() {
            restart_adbd_if_enabled()?;
            adb_restart_pending = false;
        }
        let mut redraw_area = state.refresh_status_if_due().then_some(DirtyArea::Status);
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
            }
        }

        match action {
            PowerAction::Sleep => sleep_cycle(
                &mut display,
                &mut wake_lock,
                &mut inputs,
                &mut state,
                suspend_mode,
            )
            .map(|()| {
                adb_restart_pending = true;
            })?,
            PowerAction::Reboot | PowerAction::PowerOff => {
                state.mode = "REBOOTING";
                state.message = match action {
                    PowerAction::Reboot => "REBOOT REQUESTED",
                    PowerAction::PowerOff => "POWER OFF REQUESTED",
                    PowerAction::None | PowerAction::Sleep => unreachable!(),
                }
                .into();
                redraw(&mut display, &state, wake_lock.is_held(), DirtyArea::Full)
                    .map_err(|error| display_error("reboot redraw", error))?;
                match action {
                    PowerAction::Reboot => request_reboot()?,
                    PowerAction::PowerOff => request_poweroff()?,
                    PowerAction::None | PowerAction::Sleep => unreachable!(),
                }
                return Ok(());
            }
            PowerAction::None if let Some(area) = redraw_area => {
                redraw(&mut display, &state, wake_lock.is_held(), area)
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
    wake_lock_held: bool,
    area: DirtyArea,
) -> io::Result<()> {
    let lines = screen_lines(state, wake_lock_held);
    display::draw_screen(
        display,
        &lines,
        area.region(display),
        area.waveform(),
        area.wait_for_completion(),
        area.force_refresh(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirtyArea {
    Full,
    Status,
    Touch,
    Key,
    Power,
}

impl DirtyArea {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::Key, Self::Power) | (Self::Power, Self::Key) => Self::Power,
            (left, right) if left == right => left,
            _ => Self::Full,
        }
    }

    fn region(self, display: &NativeDisplay) -> DisplayRegion {
        let region = match self {
            Self::Full => DisplayRegion::full(display.width(), display.height()),
            // The details page keeps the touch diagnostics near the bottom
            // of the content area. Keep the update well inside the display.
            Self::Touch => DisplayRegion::new(20, 460, 560, 130),
            // Key and power diagnostics share one region so a power press can
            // update the key row, power row, and status message together.
            Self::Key | Self::Power => DisplayRegion::new(20, 535, 560, 105),
            // Status refreshes also update the clock in the header, so include
            // the header and the complete diagnostic block above the actions.
            Self::Status => DisplayRegion::new(0, 0, display.width(), 640),
        };
        region.bounded(display.width(), display.height())
    }

    fn waveform(self) -> WaveformMode {
        match self {
            Self::Touch | Self::Key | Self::Power => WaveformMode::Du,
            Self::Full | Self::Status => WaveformMode::Gc16,
        }
    }

    fn wait_for_completion(self) -> bool {
        matches!(self, Self::Full | Self::Status)
    }

    fn force_refresh(self) -> bool {
        matches!(self, Self::Full)
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
    suspend_mode: SuspendMode,
) -> io::Result<()> {
    state.mode = "SLEEPING";
    state.message = match suspend_mode {
        SuspendMode::EInk => "E-ink standby mode",
        SuspendMode::Normal => "Normal mem mode",
    }
    .into();
    eprintln!("standalone-test: drawing pre-suspend screen");
    redraw(display, state, wake_lock.is_held(), DirtyArea::Full)
        .map_err(|error| display_error("pre-suspend redraw", error))?;
    let standby_lines = screen_lines(state, wake_lock.is_held());
    let standby = display::standby_screen(
        &standby_lines,
        display.width() as usize,
        display.height() as usize,
    );
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
    redraw(display, state, wake_lock.is_held(), DirtyArea::Full)
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
    let status = Command::new("/system/bin/reboot").arg("reboot").status()?;
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
    let status = Command::new("/system/bin/reboot").arg("-p").status()?;
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
enum UiPage {
    Home,
    Details,
}

struct UiState {
    page: UiPage,
    mode: &'static str,
    message: String,
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
    last_power_duration_ms: Option<u64>,
    ignore_power_until: Option<Instant>,
    status: crate::status::StatusSnapshot,
    last_status_refresh: Instant,
}

impl UiState {
    fn new() -> Self {
        Self {
            page: UiPage::Home,
            mode: "ACTIVE",
            message: "Input ready".into(),
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
            last_power_duration_ms: None,
            ignore_power_until: None,
            status: crate::status::collect(),
            last_status_refresh: Instant::now(),
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

    fn observe(
        &mut self,
        source: InputSourceKind,
        event: RawEvent,
    ) -> (Option<DirtyArea>, PowerAction) {
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
                    return (Some(DirtyArea::Full), action);
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
                    return (Some(DirtyArea::Full), action);
                }
                return (Some(DirtyArea::Touch), PowerAction::None);
            }
            return (None, PowerAction::None);
        }

        if event.event_type == EVENT_KEY {
            self.last_key = Some((source, event));
            self.key_events += 1;
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

    fn activate_tap(&mut self) -> PowerAction {
        let y = self.touch_y;
        if y < display::STATUS_BAR_HEIGHT as i32 {
            self.page = match self.page {
                UiPage::Home => UiPage::Details,
                UiPage::Details => UiPage::Home,
            };
            self.message = match self.page {
                UiPage::Home => "Returned to reading".into(),
                UiPage::Details => "Details open".into(),
            };
            return PowerAction::None;
        }
        if self.page != UiPage::Details {
            return PowerAction::None;
        }
        let within_action_x = self.touch_x >= display::DETAILS_ACTION_MARGIN as i32
            && self.touch_x
                < (display::SCREEN_WIDTH.saturating_sub(display::DETAILS_ACTION_MARGIN)) as i32;
        if !within_action_x {
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
        InputSourceKind, SuspendMode, UiPage, UiState, ABS_MT_POSITION_X, ABS_MT_POSITION_Y,
        ABS_MT_TOUCH_MAJOR, ABS_MT_TRACKING_ID, ABS_X, ABS_Y, BTN_TOUCH, EVENT_ABS, EVENT_KEY,
        EVENT_SYN, SYN_REPORT,
    };
    use crate::input::RawEvent;

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
    fn active_mode_is_hidden_from_status_bar() {
        let state = UiState::new();
        let lines = super::screen_lines(&state, true);
        assert_eq!(lines[0].split('|').nth(4), Some(""));
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
