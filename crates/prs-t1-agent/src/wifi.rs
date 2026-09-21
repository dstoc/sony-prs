use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::status;
use tokio::net::UnixDatagram as TokioUnixDatagram;
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::time::{sleep, timeout};

pub const WIFI_HELPER: &str = "/data/local/tmp/prs-t1-wifi-helper";
const WIFI_INTERFACE: &str = "wlan0";
const WPA_DEVICE_SOCKET: &str = "/data/misc/wifi/sockets/wpa_ctrl_";
const WPA_CONTROL_SOCKET_DIRS: &[&str] = &[
    "/data/system/wpa_supplicant",
    "/data/misc/wifi/sockets",
    "/data/misc/wifi/wpa_supplicant",
];
const WPA_ANDROID_SOCKET_PREFIX: &str = "/dev/socket/wpa_";
const WPA_CLIENT_SOCKET: &str = "/data/local/tmp/prs-t1-wpa";
const DHCP_SERVICE: &str = "dhcpcd";
const DHCP_SERVICE_STATE_PROPERTY: &str = "init.svc.dhcpcd";
const ASSOCIATION_TIMEOUT: Duration = Duration::from_secs(60);
const DHCP_TIMEOUT: Duration = Duration::from_secs(30);
const DHCP_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const WPA_READ_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const STATUS_WPA_READ_TIMEOUT: Duration = Duration::from_millis(100);
static WPA_CLIENT_SOCKET_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
pub enum Operation {
    Up,
    Down,
    Probe,
}

#[derive(Debug)]
struct WifiFailure {
    stage: &'static str,
    kind: &'static str,
    detail: String,
}

impl WifiFailure {
    fn new(stage: &'static str, detail: impl Into<String>) -> Self {
        Self {
            stage,
            kind: "lifecycle_failed",
            detail: detail.into(),
        }
    }

    fn with_kind(stage: &'static str, kind: &'static str, detail: impl Into<String>) -> Self {
        Self {
            stage,
            kind,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WpaControlErrorKind {
    MissingSocket,
    PermissionDenied,
    SupplicantUnavailable,
    ReadTimeout,
    Protocol,
}

impl WpaControlErrorKind {
    fn label(self) -> &'static str {
        match self {
            Self::MissingSocket => "control_socket_missing",
            Self::PermissionDenied => "control_socket_permission_denied",
            Self::SupplicantUnavailable => "supplicant_unavailable",
            Self::ReadTimeout => "control_socket_read_timeout",
            Self::Protocol => "control_socket_protocol_error",
        }
    }
}

#[derive(Debug)]
struct WpaControlError {
    kind: WpaControlErrorKind,
    path: Option<String>,
    detail: String,
}

impl WpaControlError {
    fn new(kind: WpaControlErrorKind, path: Option<&str>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.map(str::to_owned),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for WpaControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "kind={}", self.kind.label())?;
        if let Some(path) = &self.path {
            write!(formatter, " path={path}")?;
        }
        write!(formatter, " detail={}", self.detail)
    }
}

impl Error for WpaControlError {}

#[derive(Debug)]
pub(crate) struct AssociationStatus {
    pub(crate) state: Option<String>,
    pub(crate) source: String,
}

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

/// Run one authorization/synchronization attempt with Wi-Fi enabled only for
/// the duration of that attempt. Startup failures use the same cleanup path
/// as the explicit Wi-Fi probe; an active sync always attempts shutdown.
pub(crate) fn run_sync(config: crate::sync::SyncConfig) -> io::Result<()> {
    run_sync_outcome(config).map(|_| ())
}

pub(crate) fn run_sync_outcome(
    config: crate::sync::SyncConfig,
) -> io::Result<crate::sync::SyncOutcome> {
    let mut display = crate::framebuffer::NativeDisplay::open(config.framebuffer())
        .map_err(|error| io::Error::other(format!("could not open sync display: {error}")))?;
    status::ensure_native_ownership()?;
    let progress = crate::sync::new_progress();
    let runtime = RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| io::Error::other(format!("could not create sync runtime: {error}")))?;
    let mut future = Box::pin(run_sync_outcome_async(config, progress.clone()));
    loop {
        if let Some(crate::sync::SyncProgressEvent::ApprovalUrl(url)) =
            crate::sync::take_progress(&progress)
        {
            if let Err(render_error) =
                crate::display::draw_authorization_qr(&mut display, &url, "||||SYNCING|")
            {
                drop(future);
                let cleanup = runtime.block_on(shutdown_after_sync_cancellation_async());
                return match cleanup {
                    Ok(()) => Err(render_error),
                    Err(cleanup_error) => Err(io::Error::new(
                        render_error.kind(),
                        format!(
                            "display render failed: {render_error}; Wi-Fi cleanup failed: {cleanup_error}"
                        ),
                    )),
                };
            }
        }
        let result = runtime.block_on(async {
            tokio::select! {
                result = &mut future => Some(result),
                _ = tokio::task::yield_now() => None,
            }
        });
        if let Some(crate::sync::SyncProgressEvent::ApprovalUrl(url)) =
            crate::sync::take_progress(&progress)
        {
            if let Err(render_error) =
                crate::display::draw_authorization_qr(&mut display, &url, "||||SYNCING|")
            {
                if result.is_some() {
                    return Err(render_error);
                }
                drop(future);
                let cleanup = runtime.block_on(shutdown_after_sync_cancellation_async());
                return match cleanup {
                    Ok(()) => Err(render_error),
                    Err(cleanup_error) => Err(io::Error::new(
                        render_error.kind(),
                        format!(
                            "display render failed: {render_error}; Wi-Fi cleanup failed: {cleanup_error}"
                        ),
                    )),
                };
            }
        }
        if let Some(result) = result {
            return result;
        }
    }
}

pub(crate) async fn run_sync_outcome_async(
    config: crate::sync::SyncConfig,
    progress: crate::sync::SyncProgress,
) -> io::Result<crate::sync::SyncOutcome> {
    run_sync_client_outcome_async(crate::sync::new_client(config, progress)).await
}

pub(crate) async fn run_sync_client_outcome_async(
    client: crate::sync::SyncClientHandle,
) -> io::Result<crate::sync::SyncOutcome> {
    print_operation("sync");
    print_snapshot("before");
    print_timeouts();

    if let Err(error) = bring_up_async().await {
        let startup = error.to_string();
        return match finish_failed_startup_async(error).await {
            Ok(()) => Err(io::Error::other(startup)),
            Err(cleanup) => Err(cleanup),
        };
    }
    print_snapshot("ready");
    let sync_result = crate::sync::run_active_client_outcome_async(client).await;
    let shutdown_result = shutdown_async().await;
    print_snapshot("after");

    match (sync_result, shutdown_result) {
        (Ok(outcome), Ok(())) => {
            println!("wifi.result=success");
            println!("wifi.sync_result=success");
            Ok(outcome)
        }
        (Err(error), Ok(())) => {
            println!("wifi.result=failure");
            println!("wifi.sync_result=failure");
            Err(error)
        }
        (Ok(_), Err(error)) => {
            print_failure(&error);
            Err(io::Error::other(error))
        }
        (Err(sync_error), Err(shutdown_error)) => {
            print_failure(&shutdown_error);
            Err(io::Error::other(format!(
                "sync failed: {sync_error}; Wi-Fi shutdown failed: {shutdown_error}"
            )))
        }
    }
}

pub(crate) async fn shutdown_after_sync_cancellation_async() -> io::Result<()> {
    let result = shutdown_async().await;
    print_snapshot("after_cancellation_cleanup");
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            print_failure(&error);
            Err(io::Error::other(error))
        }
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

async fn bring_up_async() -> Result<(), WifiFailure> {
    println!("wifi.stage=load_driver");
    run_helper("load-driver")?;
    println!("wifi.stage=driver_loaded");

    println!("wifi.stage=start_supplicant");
    run_helper("start-supplicant")?;
    println!("wifi.stage=supplicant_started");

    wait_for_association_async().await?;
    println!("wifi.stage=associated");

    println!("wifi.stage=start_dhcp");
    set_property("ctl.start", DHCP_SERVICE)?;
    println!("wifi.stage=dhcp_started");

    wait_for_dhcp_async().await?;
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

async fn finish_failed_startup_async(error: WifiFailure) -> io::Result<()> {
    println!("wifi.result=failure");
    print_failure(&error);
    let cleanup_result = shutdown_async().await;
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

async fn shutdown_async() -> Result<(), WifiFailure> {
    println!("wifi.stage=stop_dhcp");
    let dhcp_result = stop_dhcp_async().await;
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

async fn stop_dhcp_async() -> Result<(), WifiFailure> {
    let stop_result = set_property("ctl.stop", DHCP_SERVICE);
    let wait_result = wait_for_dhcp_service_stop_async().await;

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

async fn wait_for_dhcp_service_stop_async() -> Result<(), WifiFailure> {
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
        sleep(POLL_INTERVAL).await;
    }
}

fn dhcp_service_is_stopped(state: &str) -> bool {
    state.is_empty() || state == "stopped"
}

fn dhcp_service_is_bound(state: &str) -> bool {
    matches!(state, "BOUND" | "bound" | "ok" | "OK")
}

fn wait_for_association() -> Result<(), WifiFailure> {
    let deadline = Instant::now() + ASSOCIATION_TIMEOUT;
    let mut last_state = None;
    let mut last_control_error;
    let mut last_state_error;
    let mut last_error_kind = None;
    let mut last_source = None;

    loop {
        match read_association_status_internal(WIFI_INTERFACE, WPA_READ_TIMEOUT) {
            Ok(status) => {
                if last_source.as_deref() != Some(status.source.as_str()) {
                    println!("wifi.association_source={}", status.source);
                    last_source = Some(status.source.clone());
                }
                let Some(state) = status.state else {
                    if last_state.is_some() {
                        println!("wifi.association_state=unknown");
                        last_state = None;
                    }
                    last_control_error = Some(WpaControlError::new(
                        WpaControlErrorKind::Protocol,
                        Some(&status.source),
                        "WPA status did not include wpa_state",
                    ));
                    last_state_error = None;
                    if Instant::now() >= deadline {
                        return association_timeout(last_control_error, last_state_error);
                    }
                    thread::sleep(POLL_INTERVAL);
                    continue;
                };
                if last_state.as_deref() != Some(state.as_str()) {
                    println!("wifi.association_state={state}");
                    last_state = Some(state.clone());
                }
                if state == "COMPLETED" {
                    return Ok(());
                }
                last_control_error = None;
                last_state_error = Some((status.source, state));
            }
            Err(error) => {
                if last_state.is_some() {
                    println!("wifi.association_state=unknown");
                    last_state = None;
                }
                if last_error_kind != Some(error.kind) {
                    println!("wifi.association_error_kind={}", error.kind.label());
                    if let Some(path) = &error.path {
                        println!("wifi.association_error_path={path}");
                    }
                    last_error_kind = Some(error.kind);
                }
                if error.kind == WpaControlErrorKind::PermissionDenied {
                    return Err(WifiFailure::with_kind(
                        "association_control",
                        error.kind.label(),
                        error.to_string(),
                    ));
                }
                if supplicant_is_stopped() {
                    return Err(WifiFailure::with_kind(
                        "association_control",
                        "supplicant_crashed",
                        format!("{error}; init.svc.wpa_supplicant=stopped"),
                    ));
                }
                last_control_error = Some(error);
                last_state_error = None;
            }
        }
        if Instant::now() >= deadline {
            return association_timeout(last_control_error, last_state_error);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

async fn wait_for_association_async() -> Result<(), WifiFailure> {
    let deadline = Instant::now() + ASSOCIATION_TIMEOUT;
    let mut last_state = None;
    let mut last_control_error;
    let mut last_state_error;
    let mut last_error_kind = None;
    let mut last_source = None;

    loop {
        match read_association_status_internal_async(WIFI_INTERFACE, WPA_READ_TIMEOUT).await {
            Ok(status) => {
                if last_source.as_deref() != Some(status.source.as_str()) {
                    println!("wifi.association_source={}", status.source);
                    last_source = Some(status.source.clone());
                }
                let Some(state) = status.state else {
                    if last_state.is_some() {
                        println!("wifi.association_state=unknown");
                        last_state = None;
                    }
                    last_control_error = Some(WpaControlError::new(
                        WpaControlErrorKind::Protocol,
                        Some(&status.source),
                        "WPA status did not include wpa_state",
                    ));
                    last_state_error = None;
                    if Instant::now() >= deadline {
                        return association_timeout(last_control_error, last_state_error);
                    }
                    sleep(POLL_INTERVAL).await;
                    continue;
                };
                if last_state.as_deref() != Some(state.as_str()) {
                    println!("wifi.association_state={state}");
                    last_state = Some(state.clone());
                }
                if state == "COMPLETED" {
                    return Ok(());
                }
                last_control_error = None;
                last_state_error = Some((status.source, state));
            }
            Err(error) => {
                if last_state.is_some() {
                    println!("wifi.association_state=unknown");
                    last_state = None;
                }
                if last_error_kind != Some(error.kind) {
                    println!("wifi.association_error_kind={}", error.kind.label());
                    if let Some(path) = &error.path {
                        println!("wifi.association_error_path={path}");
                    }
                    last_error_kind = Some(error.kind);
                }
                if error.kind == WpaControlErrorKind::PermissionDenied {
                    return Err(WifiFailure::with_kind(
                        "association_control",
                        error.kind.label(),
                        error.to_string(),
                    ));
                }
                if supplicant_is_stopped() {
                    return Err(WifiFailure::with_kind(
                        "association_control",
                        "supplicant_crashed",
                        format!("{error}; init.svc.wpa_supplicant=stopped"),
                    ));
                }
                last_control_error = Some(error);
                last_state_error = None;
            }
        }
        if Instant::now() >= deadline {
            return association_timeout(last_control_error, last_state_error);
        }
        sleep(POLL_INTERVAL).await;
    }
}

fn association_timeout(
    last_control_error: Option<WpaControlError>,
    last_state_error: Option<(String, String)>,
) -> Result<(), WifiFailure> {
    if let Some((source, state)) = last_state_error {
        return Err(WifiFailure::with_kind(
            "association_timeout",
            "association_timeout",
            format!("WPA state is {state} path={source}"),
        ));
    }
    match last_control_error {
        Some(error) => Err(WifiFailure::with_kind(
            "association_timeout",
            error.kind.label(),
            error.to_string(),
        )),
        None => Err(WifiFailure::new(
            "association_timeout",
            "WPA association did not complete",
        )),
    }
}

fn supplicant_is_stopped() -> bool {
    read_property("init.svc.wpa_supplicant")
        .ok()
        .is_some_and(|state| state == "stopped")
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
                if dhcp_service_is_bound(&result) {
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

async fn wait_for_dhcp_async() -> Result<(), WifiFailure> {
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
                if dhcp_service_is_bound(&result) {
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
        sleep(POLL_INTERVAL).await;
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

pub(crate) fn read_association_status(
    interface: &str,
    timeout: Duration,
) -> io::Result<AssociationStatus> {
    read_association_status_internal(interface, timeout)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))
}

fn read_association_status_internal(
    interface: &str,
    timeout: Duration,
) -> Result<AssociationStatus, WpaControlError> {
    let deadline = Instant::now() + timeout;
    let per_path_timeout = timeout.min(Duration::from_millis(500));
    let mut errors = Vec::new();

    for remote in control_socket_candidates(interface) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match read_wpa_status_from_path(&remote, remaining.min(per_path_timeout)) {
            Ok(response) => {
                return Ok(AssociationStatus {
                    state: parse_wpa_state(&response),
                    source: remote,
                });
            }
            Err(error) => {
                if matches!(
                    error.kind,
                    WpaControlErrorKind::PermissionDenied | WpaControlErrorKind::Protocol
                ) {
                    return Err(error);
                }
                errors.push(error);
            }
        }
    }

    let kind = errors
        .iter()
        .map(|error| error.kind)
        .find(|kind| *kind == WpaControlErrorKind::ReadTimeout)
        .or_else(|| {
            errors
                .iter()
                .map(|error| error.kind)
                .find(|kind| *kind == WpaControlErrorKind::SupplicantUnavailable)
        })
        .unwrap_or(WpaControlErrorKind::MissingSocket);
    let detail = if errors.is_empty() {
        "no WPA control socket candidates were attempted".to_owned()
    } else {
        errors
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    };
    Err(WpaControlError::new(kind, None, detail))
}

async fn read_association_status_internal_async(
    interface: &str,
    timeout: Duration,
) -> Result<AssociationStatus, WpaControlError> {
    let deadline = Instant::now() + timeout;
    let per_path_timeout = timeout.min(Duration::from_millis(500));
    let mut errors = Vec::new();

    for remote in control_socket_candidates(interface) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match read_wpa_status_from_path_async(&remote, remaining.min(per_path_timeout)).await {
            Ok(response) => {
                return Ok(AssociationStatus {
                    state: parse_wpa_state(&response),
                    source: remote,
                });
            }
            Err(error) => {
                if matches!(
                    error.kind,
                    WpaControlErrorKind::PermissionDenied | WpaControlErrorKind::Protocol
                ) {
                    return Err(error);
                }
                errors.push(error);
            }
        }
    }

    let kind = errors
        .iter()
        .map(|error| error.kind)
        .find(|kind| *kind == WpaControlErrorKind::ReadTimeout)
        .or_else(|| {
            errors
                .iter()
                .map(|error| error.kind)
                .find(|kind| *kind == WpaControlErrorKind::SupplicantUnavailable)
        })
        .unwrap_or(WpaControlErrorKind::MissingSocket);
    let detail = if errors.is_empty() {
        "no WPA control socket candidates were attempted".to_owned()
    } else {
        errors
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    };
    Err(WpaControlError::new(kind, None, detail))
}

fn read_wpa_status_from_path(remote: &str, timeout: Duration) -> Result<String, WpaControlError> {
    let counter = WPA_CLIENT_SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    let local = format!("{WPA_CLIENT_SOCKET}-{}-{counter}.sock", std::process::id());
    let _ = fs::remove_file(&local);

    let result = (|| {
        let socket = UnixDatagram::bind(Path::new(&local)).map_err(|error| {
            WpaControlError::from_io(error, WpaControlErrorKind::PermissionDenied, Some(&local))
        })?;
        socket.set_read_timeout(Some(timeout)).map_err(|error| {
            WpaControlError::new(
                WpaControlErrorKind::ReadTimeout,
                Some(remote),
                format!("could not set read timeout: {error}"),
            )
        })?;
        socket.connect(remote).map_err(|error| {
            WpaControlError::from_io(
                error,
                WpaControlErrorKind::SupplicantUnavailable,
                Some(remote),
            )
        })?;
        socket.send(b"STATUS").map_err(|error| {
            WpaControlError::from_io(
                error,
                WpaControlErrorKind::SupplicantUnavailable,
                Some(remote),
            )
        })?;
        let mut response = [0_u8; 4096];
        let length = socket.recv(&mut response).map_err(|error| {
            WpaControlError::from_io(error, WpaControlErrorKind::ReadTimeout, Some(remote))
        })?;
        if length == 0 {
            return Err(WpaControlError::new(
                WpaControlErrorKind::Protocol,
                Some(remote),
                "WPA control socket returned an empty response",
            ));
        }
        Ok(String::from_utf8_lossy(&response[..length]).into_owned())
    })();
    let _ = fs::remove_file(&local);
    result
}

async fn read_wpa_status_from_path_async(
    remote: &str,
    timeout_duration: Duration,
) -> Result<String, WpaControlError> {
    let counter = WPA_CLIENT_SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    let local = format!("{WPA_CLIENT_SOCKET}-{}-{counter}.sock", std::process::id());
    let _ = fs::remove_file(&local);

    let result = async {
        let socket = TokioUnixDatagram::bind(Path::new(&local)).map_err(|error| {
            WpaControlError::from_io(error, WpaControlErrorKind::PermissionDenied, Some(&local))
        })?;
        socket.connect(remote).map_err(|error| {
            WpaControlError::from_io(
                error,
                WpaControlErrorKind::SupplicantUnavailable,
                Some(remote),
            )
        })?;
        socket.send(b"STATUS").await.map_err(|error| {
            WpaControlError::from_io(
                error,
                WpaControlErrorKind::SupplicantUnavailable,
                Some(remote),
            )
        })?;
        let mut response = [0_u8; 4096];
        let length = timeout(timeout_duration, socket.recv(&mut response))
            .await
            .map_err(|_| {
                WpaControlError::new(
                    WpaControlErrorKind::ReadTimeout,
                    Some(remote),
                    "WPA control socket read timed out",
                )
            })?
            .map_err(|error| {
                WpaControlError::from_io(error, WpaControlErrorKind::ReadTimeout, Some(remote))
            })?;
        if length == 0 {
            return Err(WpaControlError::new(
                WpaControlErrorKind::Protocol,
                Some(remote),
                "WPA control socket returned an empty response",
            ));
        }
        Ok(String::from_utf8_lossy(&response[..length]).into_owned())
    }
    .await;
    let _ = fs::remove_file(&local);
    result
}

impl WpaControlError {
    fn from_io(error: io::Error, default_kind: WpaControlErrorKind, path: Option<&str>) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::NotFound => WpaControlErrorKind::MissingSocket,
            io::ErrorKind::PermissionDenied => WpaControlErrorKind::PermissionDenied,
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => WpaControlErrorKind::ReadTimeout,
            _ => default_kind,
        };
        Self::new(kind, path, error.to_string())
    }
}

fn control_socket_candidates(interface: &str) -> Vec<String> {
    let mut candidates = vec![
        format!("{WPA_ANDROID_SOCKET_PREFIX}{interface}"),
        format!("{WPA_DEVICE_SOCKET}{interface}"),
    ];
    candidates.extend(
        WPA_CONTROL_SOCKET_DIRS
            .iter()
            .map(|directory| format!("{directory}/{interface}")),
    );
    candidates
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
        &format!("wifi.{label}.association_source"),
        snapshot.wifi.association_source.as_deref(),
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
    println!("wifi.failure_kind={}", error.kind);
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
    use super::{
        association_timeout, control_socket_candidates, dhcp_service_is_bound,
        dhcp_service_is_stopped, parse_wpa_state, WpaControlErrorKind,
    };

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

    #[test]
    fn accepts_device_and_standard_dhcp_success_values() {
        assert!(dhcp_service_is_bound("ok"));
        assert!(dhcp_service_is_bound("BOUND"));
        assert!(dhcp_service_is_bound("bound"));
        assert!(!dhcp_service_is_bound("failed"));
    }

    #[test]
    fn resolves_android_22_and_legacy_control_socket_paths() {
        assert_eq!(
            control_socket_candidates("wlan0"),
            vec![
                "/dev/socket/wpa_wlan0",
                "/data/misc/wifi/sockets/wpa_ctrl_wlan0",
                "/data/system/wpa_supplicant/wlan0",
                "/data/misc/wifi/sockets/wlan0",
                "/data/misc/wifi/wpa_supplicant/wlan0",
            ]
        );
    }

    #[test]
    fn retains_the_device_specific_control_socket_fallback() {
        assert!(control_socket_candidates("wlan0")
            .iter()
            .any(|path| path == "/data/misc/wifi/sockets/wpa_ctrl_wlan0"));
    }

    #[test]
    fn labels_control_socket_failure_classes() {
        assert_eq!(
            WpaControlErrorKind::MissingSocket.label(),
            "control_socket_missing"
        );
        assert_eq!(
            WpaControlErrorKind::PermissionDenied.label(),
            "control_socket_permission_denied"
        );
        assert_eq!(
            WpaControlErrorKind::SupplicantUnavailable.label(),
            "supplicant_unavailable"
        );
    }

    #[test]
    fn keeps_association_timeout_distinct_from_control_socket_errors() {
        let error = association_timeout(
            None,
            Some((
                "/data/system/wpa_supplicant/wlan0".into(),
                "ASSOCIATING".into(),
            )),
        )
        .expect_err("an incomplete WPA state must time out");

        assert_eq!(error.stage, "association_timeout");
        assert_eq!(error.kind, "association_timeout");
    }
}
