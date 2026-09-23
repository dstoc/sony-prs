export function logicalSizeFromReader(reader) {
  if (!reader) {
    return null;
  }
  const width = reader.logical_width();
  const height = reader.logical_height();
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
    return null;
  }
  return { width, height };
}

export function logicalPointFromPointer(
  event,
  canvas,
  logicalWidth,
  logicalHeight,
) {
  const rect = canvas.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) {
    return null;
  }

  const x = (event.clientX - rect.left) * logicalWidth / rect.width;
  const y = (event.clientY - rect.top) * logicalHeight / rect.height;
  if (x < 0 || x >= logicalWidth || y < 0 || y >= logicalHeight) {
    return null;
  }
  return { x, y };
}

export function logicalPointFromReaderPointer(event, canvas, reader) {
  const size = logicalSizeFromReader(reader);
  if (!size) {
    return null;
  }
  return logicalPointFromPointer(event, canvas, size.width, size.height);
}

export function commandForKeyboardEvent(event) {
  if (event.altKey && event.key === "ArrowLeft") {
    return "back";
  }
  if (event.key === "ArrowLeft") {
    return "previous";
  }
  if (event.key === "ArrowRight") {
    return "next";
  }
  if (event.key === "Home") {
    return "home";
  }
  if (event.key === "Backspace") {
    return "back";
  }
  return null;
}
