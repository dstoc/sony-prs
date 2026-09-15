use super::service_protocol::write_protocol_line;
use super::*;

pub(super) fn draw_ui(state: &UiState) -> io::Result<()> {
    let path = Path::new("/dev/fb0");
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut info = FbVarScreeninfo::default();
    let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut info) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if info.bits_per_pixel != 8 || info.xres == 0 || info.yres == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("unsupported framebuffer format {}bpp", info.bits_per_pixel),
        ));
    }

    let width = info.xres as usize;
    let height = info.yres as usize;
    let stride = info.xres_virtual as usize;
    let length = stride * info.yres_virtual as usize;
    let mapping = unsafe {
        mmap(
            std::ptr::null_mut(),
            length,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if mapping == MAP_FAILED {
        return Err(io::Error::last_os_error());
    }

    let pixels = unsafe { std::slice::from_raw_parts_mut(mapping.cast::<u8>(), length) };
    pixels.fill(0xff);
    fill_rect(pixels, stride, width, height, 0, 0, width, 88, 0x00);
    draw_text(pixels, stride, width, height, 32, 28, "PRS-350", 4, 0xff);
    draw_text(pixels, stride, width, height, 32, 124, "RUST UI", 5, 0x00);

    stroke_rect(
        pixels,
        stride,
        width,
        height,
        28,
        210,
        width - 56,
        136,
        0x00,
    );
    draw_text(
        pixels,
        stride,
        width,
        height,
        52,
        242,
        "SIGNED PACKAGE",
        3,
        0x00,
    );
    draw_text(
        pixels,
        stride,
        width,
        height,
        52,
        290,
        "STATUS: READY",
        3,
        0x00,
    );

    stroke_rect(
        pixels,
        stride,
        width,
        height,
        28,
        382,
        width - 56,
        136,
        0x00,
    );
    draw_text(
        pixels,
        stride,
        width,
        height,
        52,
        414,
        "FRAMEBUFFER",
        3,
        0x00,
    );
    draw_text(
        pixels,
        stride,
        width,
        height,
        52,
        462,
        "600 X 800 GRAY8",
        3,
        0x00,
    );

    draw_text(
        pixels,
        stride,
        width,
        height,
        32,
        558,
        "INPUT READY",
        3,
        0x00,
    );
    if let Some((x, y, pressed)) = state.touch {
        let touch = format!("TOUCH {} {} {}", x, y, if pressed { 1 } else { 0 });
        draw_text(pixels, stride, width, height, 32, 606, &touch, 3, 0x00);
    } else {
        draw_text(
            pixels,
            stride,
            width,
            height,
            32,
            606,
            "TOUCH WAIT",
            3,
            0x00,
        );
    }
    if let Some((code, key_state)) = state.key {
        let key = format!("KEY {} {}", code, key_state);
        draw_text(pixels, stride, width, height, 32, 654, &key, 3, 0x00);
    } else {
        draw_text(pixels, stride, width, height, 32, 654, "KEY WAIT", 3, 0x00);
    }
    draw_text(
        pixels,
        stride,
        width,
        height,
        32,
        height.saturating_sub(64),
        "USB DIAGNOSTICS",
        3,
        0x00,
    );

    let power_result = unsafe { ioctl(file.as_raw_fd(), EINKFB_SET_POWER_MODE, 1u32) };
    if power_result != 0 {
        let error = io::Error::last_os_error();
        unsafe { munmap(mapping, length) };
        return Err(io::Error::new(
            error.kind(),
            format!("set E-Ink power mode: {error}"),
        ));
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mut update = EinkUpdate {
        upmode: 3,
        orientation: 1,
        x: 0,
        y: 0,
        w: info.xres,
        h: info.yres,
    };
    let update_result = unsafe { ioctl(file.as_raw_fd(), EINKFB_UPDATE_PIC, &mut update) };
    let update_error = io::Error::last_os_error();
    let unmap_result = unsafe { munmap(mapping, length) };
    if update_result != 0 {
        return Err(io::Error::new(
            update_error.kind(),
            format!("update E-Ink picture: {update_error}"),
        ));
    }
    if unmap_result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn fill_rect(
    pixels: &mut [u8],
    stride: usize,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    rect_width: usize,
    rect_height: usize,
    value: u8,
) {
    let right = x.saturating_add(rect_width).min(width);
    let bottom = y.saturating_add(rect_height).min(height);
    for row in y.min(height)..bottom {
        let offset = row * stride + x.min(width);
        pixels[offset..row * stride + right].fill(value);
    }
}

fn stroke_rect(
    pixels: &mut [u8],
    stride: usize,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    rect_width: usize,
    rect_height: usize,
    value: u8,
) {
    fill_rect(pixels, stride, width, height, x, y, rect_width, 3, value);
    fill_rect(
        pixels,
        stride,
        width,
        height,
        x,
        y.saturating_add(rect_height).saturating_sub(3),
        rect_width,
        3,
        value,
    );
    fill_rect(pixels, stride, width, height, x, y, 3, rect_height, value);
    fill_rect(
        pixels,
        stride,
        width,
        height,
        x.saturating_add(rect_width).saturating_sub(3),
        y,
        3,
        rect_height,
        value,
    );
}

fn draw_text(
    pixels: &mut [u8],
    stride: usize,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    text: &str,
    scale: usize,
    value: u8,
) {
    let mut cursor = x;
    for byte in text.bytes() {
        if byte == b' ' {
            cursor = cursor.saturating_add(6 * scale);
            continue;
        }
        let glyph = glyph(byte);
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    fill_rect(
                        pixels,
                        stride,
                        width,
                        height,
                        cursor + column * scale,
                        y + row * scale,
                        scale,
                        scale,
                        value,
                    );
                }
            }
        }
        cursor = cursor.saturating_add(6 * scale);
    }
}

fn glyph(byte: u8) -> [u8; 7] {
    match byte {
        b'0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        b'1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        b'2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        b'3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        b'4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        b'5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        b'6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        b'7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        b'8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        b'9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b11100,
        ],
        b'-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        b':' => [
            0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000,
        ],
        b'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        b'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        b'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        b'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        b'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        b'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        b'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        b'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        b'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        b'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        b'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        b'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        b'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        b'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        b'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        b'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        b'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        b'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        b'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        b'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        b'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        b'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        b'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
        b'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        b'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        b'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        _ => [0; 7],
    }
}

pub(super) fn capture_framebuffer_to(output: &mut impl Write) -> io::Result<()> {
    let path = Path::new("/dev/fb0");
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut info = FbVarScreeninfo::default();
    let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut info) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if info.bits_per_pixel != 8 || info.xres == 0 || info.yres == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("unsupported framebuffer format {}bpp", info.bits_per_pixel),
        ));
    }
    let stride = info.xres_virtual as usize;
    let length = stride * info.yres_virtual as usize;
    let bytes = info.xres as usize * info.yres as usize;
    let mapping = unsafe {
        mmap(
            std::ptr::null_mut(),
            length,
            PROT_READ,
            MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if mapping == MAP_FAILED {
        return Err(io::Error::last_os_error());
    }

    write_protocol_line(
        output,
        &format!(
            "PRS1 OK FRAMEBUFFER width={} height={} format=gray8 bytes={}",
            info.xres, info.yres, bytes
        ),
    )?;
    let pixels = mapping.cast::<u8>();
    for y in 0..info.yres as usize {
        let row = unsafe { std::slice::from_raw_parts(pixels.add(y * stride), info.xres as usize) };
        output.write_all(row)?;
    }
    output.flush()?;
    let unmap_result = unsafe { munmap(mapping, length) };
    if unmap_result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn probe_framebuffer() -> String {
    let path = Path::new("/dev/fb0");
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) => {
            return format!("unavailable-{error}");
        }
    };

    let mut info = FbVarScreeninfo::default();
    let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut info) };
    if result != 0 {
        return format!("ioctl-failed-{}", io::Error::last_os_error());
    }
    format!(
        "{}x{}-virtual={}x{}-bpp={}-gray={}-rotate={}-rgb={}@{},{}@{},{}@{}",
        info.xres,
        info.yres,
        info.xres_virtual,
        info.yres_virtual,
        info.bits_per_pixel,
        info.grayscale,
        info.rotate,
        info.red.length,
        info.red.offset,
        info.green.length,
        info.green.offset,
        info.blue.length,
        info.blue.offset
    )
}

pub(super) fn probe_input() -> String {
    let mut devices = Vec::new();
    let directory = match fs::read_dir("/dev/input") {
        Ok(directory) => directory,
        Err(error) => {
            return format!("unavailable-{error}");
        }
    };

    for entry in directory.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("event"))
        {
            devices.push(path.display().to_string());
        }
    }
    devices.sort();
    if devices.is_empty() {
        "none".into()
    } else {
        devices.join(",")
    }
}

pub(super) fn render_test() -> String {
    let path = Path::new("/dev/fb0");
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) => return format!("unavailable-{error}"),
    };

    let mut info = FbVarScreeninfo::default();
    let result = unsafe { ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut info) };
    if result != 0 {
        return format!("ioctl-failed-{}", io::Error::last_os_error());
    }
    if info.bits_per_pixel != 8 || info.xres_virtual == 0 || info.yres_virtual == 0 {
        return format!(
            "unsupported-format-{}x{}-bpp={}",
            info.xres_virtual, info.yres_virtual, info.bits_per_pixel
        );
    }

    let stride = info.xres_virtual as usize;
    let length = stride * info.yres_virtual as usize;
    let mapping = unsafe {
        mmap(
            std::ptr::null_mut(),
            length,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if mapping == MAP_FAILED {
        return format!("mmap-failed-{}", io::Error::last_os_error());
    }

    let pixels = mapping.cast::<u8>();
    for y in 0..info.yres as usize {
        for x in 0..info.xres as usize {
            let tile = ((x / 32) + (y / 32)) % 2;
            let value = if tile == 0 { 0x00 } else { 0xff };
            unsafe { pixels.add(y * stride + x).write_volatile(value) };
        }
    }

    let power_result = unsafe { ioctl(file.as_raw_fd(), EINKFB_SET_POWER_MODE, 1u32) };
    if power_result != 0 {
        let error = io::Error::last_os_error();
        unsafe { munmap(mapping, length) };
        return format!("power-mode-failed-{error}");
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mut update = EinkUpdate {
        upmode: 3,
        orientation: 1,
        x: 0,
        y: 0,
        w: info.xres,
        h: info.yres,
    };
    let update_result = unsafe { ioctl(file.as_raw_fd(), EINKFB_UPDATE_PIC, &mut update) };
    let update_error = io::Error::last_os_error();
    let unmap_result = unsafe { munmap(mapping, length) };
    if update_result != 0 {
        return format!("refresh-failed-{}", update_error);
    }
    if unmap_result != 0 {
        return format!("munmap-failed-{}", io::Error::last_os_error());
    }
    format!("{}x{}-checkerboard", info.xres, info.yres)
}
