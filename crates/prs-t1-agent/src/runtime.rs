use crate::display;
use crate::framebuffer::NativeDisplay;
use crate::input::{EventReader, RawEvent};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const EVENT_KEY: u16 = 1;
const EVENT_SYN: u16 = 0;
const EVENT_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const ABS_MT_POSITION_X: u16 = 53;
const ABS_MT_POSITION_Y: u16 = 54;
const KEY_POWER: u16 = 116;
const LONG_PRESS_MICROS: u64 = 2_000_000;
const WAKE_LOCK_NAME: &str = "prs-t1-native-test";

pub fn run(path: &Path) -> io::Result<()> {
    let mut display = NativeDisplay::open(path)?;
    let mut wake_lock = WakeLock::open()?;
    wake_lock.acquire()?;
    let mut inputs = InputSet::open()?;
    let mut state = UiState::new();

    eprintln!(
        "standalone-test: framebuffer={}x{}; zygote must already be stopped",
        display.width(),
        display.height()
    );
    redraw(&mut display, &state, wake_lock.is_held())
        .map_err(|error| display_error("initial redraw", error))?;

    loop {
        let mut redraw_needed = false;
        let mut action = PowerAction::None;
        for source in &mut inputs.sources {
            loop {
                let Some(event) = source.reader.read_one()? else {
                    break;
                };
                let (dirty, event_action) = state.observe(source.kind, event);
                redraw_needed |= dirty;
                if event_action != PowerAction::None {
                    action = event_action;
                }
            }
        }

        match action {
            PowerAction::Sleep => sleep_cycle(&mut display, &mut wake_lock, &mut state)?,
            PowerAction::Reboot => {
                state.mode = "REBOOTING";
                state.message = "REBOOT REQUESTED".into();
                redraw(&mut display, &state, wake_lock.is_held())
                    .map_err(|error| display_error("reboot redraw", error))?;
                request_reboot()?;
                return Ok(());
            }
            PowerAction::None if redraw_needed => {
                redraw(&mut display, &state, wake_lock.is_held())
                    .map_err(|error| display_error("input redraw", error))?;
            }
            PowerAction::None => {}
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn redraw(display: &mut NativeDisplay, state: &UiState, wake_lock_held: bool) -> io::Result<()> {
    let touch = state
        .last_touch
        .map(|event| {
            format!(
                "TOUCH T{} C{} V{}",
                event.event_type, event.code, event.value
            )
        })
        .unwrap_or_else(|| "TOUCH NONE".into());
    let coordinates = if state.touch_seen {
        format!("TOUCH X {} Y {}", state.touch_x, state.touch_y)
    } else {
        "TOUCH X --- Y ---".into()
    };
    let key = state
        .last_key
        .map(|(source, event)| {
            format!(
                "KEY {} T{} C{} V{}",
                source.label(),
                event.event_type,
                event.code,
                event.value
            )
        })
        .unwrap_or_else(|| "KEY NONE".into());
    let power = state
        .last_power_duration_ms
        .map(|duration| format!("POWER LAST {}MS", duration))
        .unwrap_or_else(|| "POWER SHORT SLEEP LONG REBOOT".into());
    let lines = vec![
        "PRS T1 NATIVE TEST".into(),
        "ZYGOTE STOPPED".into(),
        format!("STATE {}", state.mode),
        format!("WAKE HELD {}", if wake_lock_held { "YES" } else { "NO" }),
        coordinates,
        touch,
        format!("TOUCH EVENTS {}", state.touch_events),
        key,
        format!("KEY EVENTS {}", state.key_events),
        power,
        "SHORT SLEEP LONG REBOOT".into(),
        "ADB REBOOT RECOVERY".into(),
        state.message.clone(),
    ];
    display::draw_screen(display, &lines)
}

fn display_error(stage: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{stage}: {error}"))
}

fn sleep_cycle(
    display: &mut NativeDisplay,
    wake_lock: &mut WakeLock,
    state: &mut UiState,
) -> io::Result<()> {
    state.mode = "SLEEPING";
    state.message = "RELEASE WAKE LOCK".into();
    eprintln!("standalone-test: drawing pre-suspend screen");
    redraw(display, state, wake_lock.is_held())
        .map_err(|error| display_error("pre-suspend redraw", error))?;
    display.prepare_for_suspend();

    eprintln!("standalone-test: requesting suspend");
    let sleep_result = wake_lock.release().and_then(|_| request_suspend());
    eprintln!("standalone-test: suspend request returned: {sleep_result:?}");
    let acquire_result = wake_lock.acquire();
    eprintln!("standalone-test: wake lock reacquire returned: {acquire_result:?}");
    let resume_result = display.resume_after_suspend();
    eprintln!("standalone-test: framebuffer remap returned: {resume_result:?}");

    sleep_result?;
    acquire_result?;
    resume_result.map_err(|error| display_error("post-resume framebuffer remap", error))?;

    state.mode = "ACTIVE";
    state.message = "WOKE - INPUT READY".into();
    state.last_power_duration_ms = None;
    state.ignore_power_until = Some(Instant::now() + Duration::from_secs(2));
    redraw(display, state, wake_lock.is_held())
        .map_err(|error| display_error("post-resume redraw", error))
}

fn request_suspend() -> io::Result<()> {
    let mut state = OpenOptions::new().write(true).open("/sys/power/state")?;
    state.write_all(b"mem\n")?;
    state.flush()
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
}

struct UiState {
    mode: &'static str,
    message: String,
    touch_seen: bool,
    touch_x: i32,
    touch_y: i32,
    last_touch: Option<RawEvent>,
    last_key: Option<(InputSourceKind, RawEvent)>,
    touch_events: u64,
    key_events: u64,
    power_press_us: Option<u64>,
    last_power_duration_ms: Option<u64>,
    ignore_power_until: Option<Instant>,
}

impl UiState {
    fn new() -> Self {
        Self {
            mode: "ACTIVE",
            message: "INPUT READY".into(),
            touch_seen: false,
            touch_x: 0,
            touch_y: 0,
            last_touch: None,
            last_key: None,
            touch_events: 0,
            key_events: 0,
            power_press_us: None,
            last_power_duration_ms: None,
            ignore_power_until: None,
        }
    }

    fn observe(&mut self, source: InputSourceKind, event: RawEvent) -> (bool, PowerAction) {
        if source == InputSourceKind::Touch {
            self.last_touch = Some(event);
            if event.event_type == EVENT_ABS && event.code == ABS_MT_POSITION_X {
                self.touch_x = event.value;
                self.touch_seen = true;
            }
            if event.event_type == EVENT_ABS && event.code == ABS_MT_POSITION_Y {
                self.touch_y = event.value;
                self.touch_seen = true;
            }
            if event.event_type == EVENT_SYN && event.code == SYN_REPORT {
                self.touch_events += 1;
                return (true, PowerAction::None);
            }
            return (false, PowerAction::None);
        }

        if event.event_type == EVENT_KEY {
            self.last_key = Some((source, event));
            self.key_events += 1;
            if source.is_power() && event.code == KEY_POWER {
                return self.observe_power(event);
            }
            return (true, PowerAction::None);
        }

        (false, PowerAction::None)
    }

    fn observe_power(&mut self, event: RawEvent) -> (bool, PowerAction) {
        if let Some(deadline) = self.ignore_power_until {
            if Instant::now() < deadline {
                if event.value == 0 {
                    self.ignore_power_until = None;
                }
                self.message = "WAKE POWER IGNORED".into();
                return (true, PowerAction::None);
            }
            self.ignore_power_until = None;
        }
        match event.value {
            1 => {
                if self.power_press_us.is_none() {
                    self.power_press_us = Some(event.timestamp_micros());
                }
                self.message = "POWER HELD".into();
                (true, PowerAction::None)
            }
            0 => {
                let duration = self
                    .power_press_us
                    .take()
                    .map(|start| event.timestamp_micros().saturating_sub(start))
                    .unwrap_or_default();
                self.last_power_duration_ms = Some(duration / 1_000);
                if duration >= LONG_PRESS_MICROS {
                    self.message = "LONG POWER - REBOOT".into();
                    (true, PowerAction::Reboot)
                } else {
                    self.message = "SHORT POWER - SLEEP".into();
                    (true, PowerAction::Sleep)
                }
            }
            2 => {
                self.message = "POWER REPEAT".into();
                (true, PowerAction::None)
            }
            _ => (true, PowerAction::None),
        }
    }
}
