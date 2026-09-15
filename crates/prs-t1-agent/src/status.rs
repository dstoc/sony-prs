use std::fmt::Display;
use std::fs;
use std::path::Path;
use std::process::Command;

const BATTERY_PATH: &str = "/sys/class/power_supply/sub_cpu_battery";
const AC_PATH: &str = "/sys/class/power_supply/sub_cpu_ac";
const USB_POWER_PATH: &str = "/sys/class/power_supply/sub_cpu_usb";
const DEFAULT_WIFI_INTERFACE: &str = "wlan0";

#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    pub uptime_seconds: Option<f64>,
    pub battery: BatteryStatus,
    pub power: PowerStatus,
    pub usb: UsbStatus,
    pub wifi: WifiStatus,
    pub adb: AdbStatus,
    pub android: AndroidStatus,
}

#[derive(Debug, Clone, Default)]
pub struct BatteryStatus {
    pub available: bool,
    pub status: Option<String>,
    pub health: Option<String>,
    pub capacity_percent: Option<i64>,
    pub voltage_uv: Option<i64>,
    pub temperature: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct PowerStatus {
    pub ac_online: Option<bool>,
    pub usb_online: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct UsbStatus {
    pub physical_connected: Option<bool>,
    pub gadget_state: Option<String>,
    pub gadget_functions: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct WifiStatus {
    pub interface: String,
    pub interface_present: bool,
    pub operstate: Option<String>,
    pub carrier: Option<bool>,
    pub signal_dbm: Option<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct AdbStatus {
    pub persist_enabled: Option<bool>,
    pub service_state: Option<String>,
    pub process_running: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AndroidStatus {
    pub zygote_running: bool,
    pub system_server_running: bool,
    pub dispd_running: bool,
}

pub fn collect() -> StatusSnapshot {
    let wifi_interface =
        read_property("wifi.interface").unwrap_or_else(|| DEFAULT_WIFI_INTERFACE.into());
    let wifi_path = format!("/sys/class/net/{wifi_interface}");

    let uptime_seconds = read_first_number("/proc/uptime");
    let battery = BatteryStatus {
        available: Path::new(BATTERY_PATH).is_dir(),
        status: read_field(BATTERY_PATH, "status"),
        health: read_field(BATTERY_PATH, "health"),
        capacity_percent: read_field(BATTERY_PATH, "capacity").and_then(|value| value.parse().ok()),
        voltage_uv: read_field(BATTERY_PATH, "voltage_now").and_then(|value| value.parse().ok()),
        temperature: read_field(BATTERY_PATH, "temp").and_then(|value| value.parse().ok()),
    };
    let power = PowerStatus {
        ac_online: read_bool_field(AC_PATH, "online"),
        usb_online: read_bool_field(USB_POWER_PATH, "online"),
    };
    let usb = UsbStatus {
        physical_connected: read_bool_field(USB_POWER_PATH, "online"),
        gadget_state: read_field("/sys/class/android_usb/android0", "state")
            .or_else(|| read_property("sys.usb.state")),
        gadget_functions: read_field("/sys/class/android_usb/android0", "functions")
            .or_else(|| read_property("sys.usb.config")),
    };
    let wifi = WifiStatus {
        interface_present: Path::new(&wifi_path).is_dir(),
        operstate: read_field(&wifi_path, "operstate"),
        carrier: read_bool_field(&wifi_path, "carrier"),
        signal_dbm: read_wireless_signal(&wifi_interface),
        interface: wifi_interface,
    };
    let processes = process_snapshot();
    let adb = AdbStatus {
        persist_enabled: read_property("persist.service.adb.enable")
            .and_then(|value| parse_bool(&value)),
        service_state: read_property("init.svc.adbd"),
        process_running: process_running(processes.as_deref(), "adbd"),
    };
    let android = AndroidStatus {
        zygote_running: process_running(processes.as_deref(), "zygote"),
        system_server_running: process_running(processes.as_deref(), "system_server"),
        dispd_running: process_running(processes.as_deref(), "dispd"),
    };

    StatusSnapshot {
        uptime_seconds,
        battery,
        power,
        usb,
        wifi,
        adb,
        android,
    }
}

pub fn print_status() {
    let status = collect();

    print_option("status.uptime_seconds", status.uptime_seconds);

    println!("battery.available={}", status.battery.available);
    print_option("battery.status", status.battery.status);
    print_option("battery.health", status.battery.health);
    print_option("battery.capacity_percent", status.battery.capacity_percent);
    print_option("battery.voltage_uv", status.battery.voltage_uv);
    print_option("battery.temperature", status.battery.temperature);

    print_option("power.ac_online", status.power.ac_online);
    print_option("power.usb_online", status.power.usb_online);

    print_option("usb.physical_connected", status.usb.physical_connected);
    print_option("usb.gadget_state", status.usb.gadget_state);
    print_option("usb.gadget_functions", status.usb.gadget_functions);

    println!("wifi.interface={}", status.wifi.interface);
    println!("wifi.interface_present={}", status.wifi.interface_present);
    print_option("wifi.operstate", status.wifi.operstate);
    print_option("wifi.carrier", status.wifi.carrier);
    print_option("wifi.signal_dbm", status.wifi.signal_dbm);

    print_option("adb.persist_enabled", status.adb.persist_enabled);
    print_option("adb.service_state", status.adb.service_state);
    println!("adb.process_running={}", status.adb.process_running);

    println!("android.zygote_running={}", status.android.zygote_running);
    println!(
        "android.system_server_running={}",
        status.android.system_server_running
    );
    println!("android.dispd_running={}", status.android.dispd_running);
}

fn print_option<T: Display>(key: &str, value: Option<T>) {
    match value {
        Some(value) => println!("{key}={value}"),
        None => println!("{key}=unknown"),
    }
}

fn read_field(root: &str, field: &str) -> Option<String> {
    let path = format!("{root}/{field}");
    read_trimmed(Path::new(&path))
}

fn read_bool_field(root: &str, field: &str) -> Option<bool> {
    read_field(root, field).and_then(|value| parse_bool(&value))
}

fn read_trimmed(path: &Path) -> Option<String> {
    let value = fs::read_to_string(path).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn read_first_number(path: &str) -> Option<f64> {
    read_trimmed(Path::new(path))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn read_property(name: &str) -> Option<String> {
    let output = Command::new("/system/bin/getprop")
        .arg(name)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn process_snapshot() -> Option<String> {
    let output = Command::new("/system/bin/ps").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn process_running(snapshot: Option<&str>, expected: &str) -> bool {
    snapshot.is_some_and(|snapshot| {
        snapshot.lines().any(|line| {
            line.split_whitespace()
                .any(|token| token.rsplit('/').next() == Some(expected))
        })
    })
}

fn read_wireless_signal(interface: &str) -> Option<i32> {
    let content = fs::read_to_string("/proc/net/wireless").ok()?;
    parse_wireless_signal(&content, interface)
}

fn parse_wireless_signal(content: &str, interface: &str) -> Option<i32> {
    for line in content.lines().skip(2) {
        let (name, values) = line.split_once(':')?;
        if name.trim() != interface {
            continue;
        }
        let mut fields = values.split_whitespace();
        fields.next()?;
        fields.next()?;
        let level = fields.next()?;
        return level.trim_end_matches('.').parse().ok();
    }
    None
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "1" | "true" | "TRUE" | "yes" | "on" => Some(true),
        "0" | "false" | "FALSE" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_bool, parse_wireless_signal};

    #[test]
    fn parses_boolean_sysfs_and_property_values() {
        assert_eq!(parse_bool("1"), Some(true));
        assert_eq!(parse_bool("off"), Some(false));
        assert_eq!(parse_bool("not-a-bool"), None);
    }

    #[test]
    fn parses_wireless_signal_level() {
        let content =
            "Inter-| sta-| Quality\n face | ...\n wlan0: 0000 70. -42. -256. 0 0 0 0 0 0 0\n";
        assert_eq!(parse_wireless_signal(content, "wlan0"), Some(-42));
    }
}
