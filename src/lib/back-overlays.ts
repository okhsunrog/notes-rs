// Overlay dismissal takes precedence over workspace navigation. Keep a stack so
// a nested overlay consumes one Back press without also dismissing its parent.
const overlays: Array<() => void> = [];

export function registerBackOverlay(close: () => void) {
  overlays.push(close);
  return () => {
    const index = overlays.lastIndexOf(close);
    if (index !== -1) overlays.splice(index, 1);
  };
}

export function dismissBackOverlay(): boolean {
  const close = overlays[overlays.length - 1];
  if (!close) return false;
  close();
  return true;
}
