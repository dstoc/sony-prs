import { logicalSizeFromReader } from "./input.mjs";

export function containedSize(
  availableWidth,
  availableHeight,
  logicalWidth,
  logicalHeight,
) {
  if (
    !Number.isFinite(availableWidth)
    || !Number.isFinite(availableHeight)
    || !Number.isFinite(logicalWidth)
    || !Number.isFinite(logicalHeight)
    || availableWidth <= 0
    || availableHeight <= 0
    || logicalWidth <= 0
    || logicalHeight <= 0
  ) {
    return null;
  }

  const scale = Math.min(
    availableWidth / logicalWidth,
    availableHeight / logicalHeight,
  );
  return {
    width: logicalWidth * scale,
    height: logicalHeight * scale,
  };
}

export function syncCanvasPresentation({ canvas, readerFrame, reader, fullscreen }) {
  const size = logicalSizeFromReader(reader) ?? {
    width: canvas.width,
    height: canvas.height,
  };
  if (canvas.width !== size.width) {
    canvas.width = size.width;
  }
  if (canvas.height !== size.height) {
    canvas.height = size.height;
  }
  readerFrame.style.aspectRatio = `${size.width} / ${size.height}`;

  if (fullscreen) {
    const fitted = containedSize(
      readerFrame.clientWidth,
      readerFrame.clientHeight,
      size.width,
      size.height,
    );
    if (fitted) {
      canvas.style.width = `${fitted.width}px`;
      canvas.style.height = `${fitted.height}px`;
    }
  } else {
    canvas.style.removeProperty("width");
    canvas.style.removeProperty("height");
  }
  return size;
}

export function createBrowserFullscreenController({
  target,
  documentObject,
  windowObject,
  onFullscreenChange,
  onViewportResize,
  setStatus,
}) {
  const isFullscreen = () => documentObject.fullscreenElement === target;
  const refresh = () => onFullscreenChange(isFullscreen());
  const onFullscreenChanged = () => refresh();

  documentObject.addEventListener("fullscreenchange", onFullscreenChanged);
  windowObject.addEventListener("resize", onViewportResize);
  windowObject.addEventListener("orientationchange", onViewportResize);
  refresh();

  async function toggle() {
    if (isFullscreen()) {
      if (typeof documentObject.exitFullscreen !== "function") {
        setStatus("Browser fullscreen exit is not supported.");
        return false;
      }
      try {
        await documentObject.exitFullscreen();
        refresh();
        return true;
      } catch (error) {
        setStatus(`Could not exit browser fullscreen: ${errorMessage(error)}`);
        return false;
      }
    }

    if (typeof target.requestFullscreen !== "function") {
      setStatus("Browser fullscreen is not supported in this browser.");
      return false;
    }
    try {
      await target.requestFullscreen();
      refresh();
      return true;
    } catch (error) {
      setStatus(`Could not enter browser fullscreen: ${errorMessage(error)}`);
      return false;
    }
  }

  function dispose() {
    documentObject.removeEventListener("fullscreenchange", onFullscreenChanged);
    windowObject.removeEventListener("resize", onViewportResize);
    windowObject.removeEventListener("orientationchange", onViewportResize);
  }

  return { isFullscreen, refresh, toggle, dispose };
}

function errorMessage(error) {
  return error && typeof error.message === "string" ? error.message : String(error);
}
