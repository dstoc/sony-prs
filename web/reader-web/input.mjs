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
