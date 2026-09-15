use super::*;

pub(super) fn watch_usb(path: Option<String>) {
    let Some(path) = path else {
        return;
    };
    let result = (|| -> io::Result<()> {
        let mut armed = false;
        loop {
            let file = match OpenOptions::new()
                .read(true)
                .custom_flags(O_NONBLOCK)
                .open(&path)
            {
                Ok(file) => file,
                Err(_) => {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    continue;
                }
            };
            let mut descriptor = PollFd {
                fd: file.as_raw_fd(),
                events: 0,
                revents: 0,
            };
            let result = unsafe { poll(&mut descriptor, 1, 1000) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if descriptor.revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
                if armed {
                    let _ = Command::new("/bin/sync").status();
                    let _ = Command::new("/sbin/reboot").status();
                    return Ok(());
                }
                continue;
            }
            armed = true;
        }
    })();
    if let Err(error) = result {
        eprintln!("USB recovery watcher stopped: {error}");
    }
}
