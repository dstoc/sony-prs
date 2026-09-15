use std::fs;
use std::process::Command;

pub fn print_inventory() {
    println!("android_processes=");
    match Command::new("/system/bin/ps").output() {
        Ok(output) if output.status.success() => {
            for line in String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| {
                    line.contains("adbd")
                        || line.contains("zygote")
                        || line.contains("system_server")
                        || line.contains("surfaceflinger")
                        || line.contains("SurfaceFlinger")
                        || line.contains("dispd")
                        || line.contains("wpa_supplicant")
                        || line.contains("netd")
                        || line.contains("dhcpcd")
                        || line.contains("launcher")
                        || line.contains("reader")
                })
            {
                println!("  {line}");
            }
        }
        Ok(output) => println!(
            "  ps_failed=status={} stderr={}",
            output.status,
            display_cmdline(&output.stderr)
        ),
        Err(error) => println!("  ps_unavailable={error}"),
    }

    println!("android_services=");
    for path in ["/init.rc", "/init.goldfish.rc", "/init.usb.rc"] {
        match fs::read_to_string(path) {
            Ok(content) => {
                println!("  init_file={path} bytes={}", content.len());
                for line in content.lines().filter(|line| {
                    line.contains("service ")
                        || line.contains("adbd")
                        || line.contains("zygote")
                        || line.contains("surfaceflinger")
                        || line.contains("dispd")
                        || line.contains("wpa_supplicant")
                }) {
                    println!("    {line}");
                }
            }
            Err(error) => println!("  init_file={path} unavailable={error}"),
        }
    }

    println!("android_framebuffer_proc=");
    match fs::read_to_string("/proc/fb") {
        Ok(content) => print_indented(&content),
        Err(error) => println!("  unavailable={error}"),
    }
}

fn display_cmdline(bytes: &[u8]) -> String {
    let mut value = String::from_utf8_lossy(bytes).replace('\0', " ");
    while value.ends_with(' ') {
        value.pop();
    }
    if value.is_empty() {
        "<empty>".into()
    } else {
        value
    }
}

fn print_indented(content: &str) {
    for line in content.lines() {
        println!("  {line}");
    }
}
