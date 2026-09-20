use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::status;

pub const WIFI_HELPER: &str = "/data/local/tmp/prs-t1-wifi-helper";
const WIFI_INTERFACE: &str = "wlan0";
const WPA_CONTROL_SOCKET: &str = "/data/misc/wifi/sockets/wpa_ctrl_";
const WPA_CLIENT_SOCKET: &str = "/data/local/tmp/prs-t1-wpa";
const DHCP_SERVICE: &str = "dhcpcd";
const DHCP_SERVICE_STATE_PROPERTY: &str = "init.svc.dhcpcd";
const ASSOCIATION_TIMEOUT: Duration = Duration::from_secs(60);
const DHCP_TIMEOUT: Duration = Duration::from_secs(30);
const DHCP_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const WPA_READ_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const STATUS_WPA_READ_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy)]
pub enum Operation {
    Up,
    Down,
    Probe,
}

#[derive(Debug)]
struct WifiFailure {
    stage: &'static str,
    detail: String,
}

impl WifiFailure {
    fn new(stage: &'static str, detail: impl Into<String>) -> Self {
        Self {
            stage,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for WifiFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.stage, self.detail)
    }
}

impl Error for WifiFailure {}

pub fn run(operation: Operation, args: Vec<String>) -> io::Result<()> {
    match operation {
        Operation::Up => {
            reject_arguments(&args, "wifi-up accepts no arguments")?;
            run_up()
        }
        Operation::Down => {
            reject_arguments(&args, "wifi-down accepts no arguments")?;
            run_down()
        }
        Operation::Probe => run_probe(args),
    }
}

fn run_up() -> io::Result<()> {
    print_operation("up");
    print_snapshot("before");
    print_timeouts();

    match bring_up() {
        Ok(()) => {
            print_snapshot("after_up");
            println!("wifi.result=success");
            Ok(())
        }
        Err(error) => finish_failed_startup(error),
    }
}

fn run_down() -> io::Result<()> {
    print_operation("down");
    print_snapshot("before");
    println!(
        "wifi.dhcp_stop_timeout_seconds={}",
        DHCP_STOP_TIMEOUT.as_secs()
    );
    let result = shutdown();
    print_snapshot("after");
    match result {
        Ok(()) => {
            println!("wifi.result=success");
            Ok(())
        }
        Err(error) => {
            print_failure(&error);
            Err(io::Error::other(error))
        }
    }
}

fn run_probe(args: Vec<String>) -> io::Result<()> {
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "wifi-probe requires the same HTTPS endpoint arguments as network-probe",
        ));
    }

    print_operation("probe");
    print_snapshot("before");
    print_timeouts();

    if let Err(error) = bring_up() {
        return finish_failed_startup(error);
    }

    print_snapshot("ready");
    let probe_result = crate::network::run(args);
    let shutdown_result = shutdown();
    print_snapshot("after");

    match (probe_result, shutdown_result) {
        (Ok(()), Ok(())) => {
            println!("wifi.result=success");
            println!("wifi.probe_result=success");
            Ok(())
        }
        (Err(error), Ok(())) => {
            println!("wifi.result=failure");
            println!("wifi.probe_result=failure");
            Err(error)
        }
        (Ok(()), Err(error)) => {
            print_failure(&error);
            Err(io::Error::other(error))
        }
        (Err(probe_error), Err(shutdown_error)) => {
            print_failure(&shutdown_error);
            Err(io::Error::other(format!(
                "network probe failed: {probe_error}; Wi-Fi shutdown failed: {shutdown_error}"
            )))
        }
    }
}

fn print_timeouts() {
    println!(
        "wifi.association_timeout_seconds={}",
        ASSOCIATION_TIMEOUT.as_secs()
    );
    println!("wifi.dhcp_timeout_seconds={}", DHCP_TIMEOUT.as_secs());
    println!(
        "wifi.dhcp_stop_timeout_seconds={}",
        DHCP_STOP_TIMEOUT.as_secs()
    );
}

fn bring_up() -> Result<(), WifiFailure> {
    println!("wifi.stage=load_driver");
    run_helper("load-driver")?;
    println!("wifi.stage=driver_loaded");

    println!("wifi.stage=start_supplicant");
    run_helper("start-supplicant")?;
    println!("wifi.stage=supplicant_started");

    wait_for_association()?;
    println!("wifi.stage=associated");

    println!("wifi.stage=start_dhcp");
    set_property("ctl.start", DHCP_SERVICE)?;
    println!("wifi.stage=dhcp_started");

    wait_for_dhcp()?;
    println!("wifi.stage=ready");
    Ok(())
}

fn finish_failed_startup(error: WifiFailure) -> io::Result<()> {
    println!("wifi.result=failure");
    print_failure(&error);
    let cleanup_result = shutdown();
    print_snapshot("after_failure_cleanup");
    match cleanup_result {
        Ok(()) => Err(io::Error::other(error)),
        Err(cleanup_error) => Err(io::Error::other(format!(
            "Wi-Fi startup failed: {error}; cleanup failed: {cleanup_error}"
        ))),
    }
}

fn shutdown() -> Result<(), WifiFailure> {
    println!("wifi.stage=stop_dhcp");
    let dhcp_result = stop_dhcp();
    print_shutdown_step("stop_dhcp", &dhcp_result);

    println!("wifi.stage=stop_supplicant");
    let supplicant_result = run_helper("stop-supplicant");
    print_shutdown_step("stop_supplicant", &supplicant_result);

    println!("wifi.stage=unload_driver");
    let driver_result = run_helper("unload-driver");
    print_shutdown_step("unload_driver", &driver_result);

    let failures = [
        dhcp_result.err(),
        supplicant_result.err(),
        driver_result.err(),
    ]
    .into_iter()
    .flatten()
    .map(|error| error.to_string())
    .collect::<Vec<_>>();
    if failures.is_empty() {
        println!("wifi.stage=off");
        Ok(())
    } else {
        Err(WifiFailure::new("shutdown", failures.join("; ")))
    }
}

fn stop_dhcp() -> Result<(), WifiFailure> {
    let stop_result = set_property("ctl.stop", DHCP_SERVICE);
    let wait_result = wait_for_dhcp_service_stop();

    match (stop_result, wait_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(stop_error), Ok(())) => Err(stop_error),
        (Ok(()), Err(wait_error)) => Err(wait_error),
        (Err(stop_error), Err(wait_error)) => Err(WifiFailure::new(
            "dhcp_stop",
            format!("{stop_error}; {wait_error}"),
        )),
    }
}

fn wait_for_dhcp_service_stop() -> Result<(), WifiFailure> {
    let deadline = Instant::now() + DHCP_STOP_TIMEOUT;
    let mut last_state = None;

    loop {
        match read_property(DHCP_SERVICE_STATE_PROPERTY) {
            Ok(state) if dhcp_service_is_stopped(&state) => {
                println!(
                    "wifi.dhcp_service_state={}",
                    if state.is_empty() {
                        "absent"
                    } else {
                        "stopped"
                    }
                );
                return Ok(());
            }
            Ok(state) => {
                if last_state.as_deref() != Some(state.as_str()) {
                    println!("wifi.dhcp_service_state={state}");
                    last_state = Some(state);
                }
            }
            Err(error) => {
                if last_state.is_some() {
                    println!("wifi.dhcp_service_state=unknown");
                    last_state = None;
                }
                if Instant::now() >= deadline {
                    return Err(WifiFailure::new(
                        "dhcp_stop_timeout",
                        format!("could not read {DHCP_SERVICE_STATE_PROPERTY}: {error}"),
                    ));
                }
            }
        }

        if Instant::now() >= deadline {
            return Err(WifiFailure::new(
                "dhcp_stop_timeout",
                format!(
                    "{DHCP_SERVICE_STATE_PROPERTY} did not become stopped or absent (last={})",
                    last_state.as_deref().unwrap_or("unknown")
                ),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn dhcp_service_is_stopped(state: &str) -> bool {
    state.is_empty() || state == "stopped"
}

fn wait_for_association() -> Result<(), WifiFailure> {
    let deadline = Instant::now() + ASSOCIATION_TIMEOUT;
    let mut last_state = None;
    let mut last_error;

    loop {
        match read_association_state(WIFI_INTERFACE, WPA_READ_TIMEOUT) {
            Ok(Some(state)) => {
                if last_state.as_deref() != Some(state.as_str()) {
                    println!("wifi.association_state={state}");
                    last_state = Some(state.clone());
                }
                if state == "COMPLETED" {
                    return Ok(());
                }
                last_error = Some(format!("WPA state is {state}"));
            }
            Ok(None) => {
                if last_state.is_some() {
                    println!("wifi.association_state=unknown");
                    last_state = None;
                }
                last_error = Some("WPA status did not include wpa_state".into());
            }
            Err(error) => {
                if last_state.is_some() {
                    println!("wifi.association_state=unknown");
                    last_state = None;
                }
                last_error = Some(format!("could not read WPA status: {error}"));
            }
        }
        if Instant::now() >= deadline {
            return Err(WifiFailure::new(
                "association_timeout",
                last_error.unwrap_or_else(|| "WPA association did not complete".into()),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_dhcp() -> Result<(), WifiFailure> {
    let deadline = Instant::now() + DHCP_TIMEOUT;
    let mut last_result = None;

    loop {
        match read_property("dhcp.wlan0.result") {
            Ok(result) => {
                if last_result.as_deref() != Some(result.as_str()) {
                    println!(
                        "wifi.dhcp_result={}",
                        if result.is_empty() {
                            "unknown"
                        } else {
                            &result
                        }
                    );
                    last_result = Some(result.clone());
                }
                if result == "BOUND" {
                    return Ok(());
                }
            }
            Err(error) => {
                if last_result.is_some() {
                    println!("wifi.dhcp_result=unknown");
                    last_result = None;
                }
                if Instant::now() >= deadline {
                    return Err(WifiFailure::new(
                        "dhcp_timeout",
                        format!("could not read dhcp.wlan0.result: {error}"),
                    ));
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(WifiFailure::new(
                "dhcp_timeout",
                format!(
                    "dhcp.wlan0.result did not become BOUND (last={})",
                    last_result.as_deref().unwrap_or("unknown")
                ),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn run_helper(action: &str) -> Result<(), WifiFailure> {
    let status = Command::new(WIFI_HELPER)
        .arg(action)
        .status()
        .map_err(|error| WifiFailure::new("helper", format!("could not run {action}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(WifiFailure::new(
            "helper",
            format!("{action} exited with {status}"),
        ))
    }
}

fn set_property(name: &str, value: &str) -> Result<(), WifiFailure> {
    let status = Command::new("/system/bin/setprop")
        .args([name, value])
        .status()
        .map_err(|error| WifiFailure::new("property", format!("could not set {name}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(WifiFailure::new(
            "property",
            format!("setprop {name} exited with {status}"),
        ))
    }
}

fn read_property(name: &str) -> io::Result<String> {
    let output = Command::new("/system/bin/getprop").arg(name).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "getprop {name} exited with {}",
            output.status
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(crate) fn read_association_state(
    interface: &str,
    timeout: Duration,
) -> io::Result<Option<String>> {
    read_wpa_status(interface, timeout).map(|response| parse_wpa_state(&response))
}

fn read_wpa_status(interface: &str, timeout: Duration) -> io::Result<String> {
    let remote = format!("{WPA_CONTROL_SOCKET}{interface}");
    let local = format!("{WPA_CLIENT_SOCKET}-{}.sock", std::process::id());
    let _ = fs::remove_file(&local);

    let result = (|| {
        let socket = UnixDatagram::bind(Path::new(&local))?;
        socket.set_read_timeout(Some(timeout))?;
        socket.connect(remote)?;
        socket.send(b"STATUS")?;
        let mut response = [0_u8; 4096];
        let length = socket.recv(&mut response)?;
        Ok(String::from_utf8_lossy(&response[..length]).into_owned())
    })();
    let _ = fs::remove_file(&local);
    result
}

fn parse_wpa_state(response: &str) -> Option<String> {
    response.lines().find_map(|line| {
        let value = line.strip_prefix("wpa_state=")?.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn print_operation(operation: &str) {
    println!("probe=wifi");
    println!("wifi.operation={operation}");
    println!("wifi.interface={WIFI_INTERFACE}");
}

fn print_snapshot(label: &str) {
    let snapshot = status::collect();
    println!("wifi.snapshot={label}");
    println!(
        "wifi.{label}.interface_present={}",
        snapshot.wifi.interface_present
    );
    print_optional(
        &format!("wifi.{label}.operstate"),
        snapshot.wifi.operstate.as_deref(),
    );
    let carrier = snapshot.wifi.carrier.map(|value| value.to_string());
    print_optional(&format!("wifi.{label}.carrier"), carrier.as_deref());
    print_optional(
        &format!("wifi.{label}.association_state"),
        snapshot.wifi.association_state.as_deref(),
    );
    print_optional(
        &format!("wifi.{label}.dhcp_result"),
        snapshot.wifi.dhcp_result.as_deref(),
    );
}

fn print_optional(key: &str, value: Option<&str>) {
    println!("{key}={}", value.unwrap_or("unknown"));
}

fn print_shutdown_step(step: &str, result: &Result<(), WifiFailure>) {
    match result {
        Ok(()) => println!("wifi.shutdown.{step}=success"),
        Err(error) => println!("wifi.shutdown.{step}=failure error={error}"),
    }
}

fn print_failure(error: &WifiFailure) {
    println!("wifi.failure_stage={}", error.stage);
    println!("wifi.failure_kind=lifecycle_failed");
    println!("wifi.error={}", error.detail);
}

fn reject_arguments(args: &[String], message: &str) -> io::Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidInput, message))
    }
}

#[cfg(test)]
mod tests {
    use super::{dhcp_service_is_stopped, parse_wpa_state};

    #[test]
    fn parses_only_the_wpa_state_field() {
        let response = "bssid=aa:bb:cc:dd:ee:ff\nssid=private-network\nwpa_state=COMPLETED\nip_address=192.0.2.1\n";
        assert_eq!(parse_wpa_state(response), Some("COMPLETED".into()));
    }

    #[test]
    fn reports_missing_wpa_state_without_exposing_other_fields() {
        let response = "ssid=private-network\nkey_mgmt=WPA-PSK\n";
        assert_eq!(parse_wpa_state(response), None);
    }

    #[test]
    fn treats_stopped_or_absent_dhcp_service_as_off() {
        assert!(dhcp_service_is_stopped("stopped"));
        assert!(dhcp_service_is_stopped(""));
    }

    #[test]
    fn waits_for_active_dhcp_service_states() {
        assert!(!dhcp_service_is_stopped("running"));
        assert!(!dhcp_service_is_stopped("stopping"));
    }
}
