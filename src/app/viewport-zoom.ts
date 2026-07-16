export function disableViewportZoom() {
  const preventWheelZoom = (event: WheelEvent) => {
    if (event.ctrlKey || event.metaKey) event.preventDefault();
  };
  const preventKeyboardZoom = (event: KeyboardEvent) => {
    if (!(event.ctrlKey || event.metaKey)) return;
    if (["+", "-", "=", "0"].includes(event.key)) event.preventDefault();
  };
  const preventGestureZoom = (event: Event) => event.preventDefault();

  window.addEventListener("wheel", preventWheelZoom, { passive: false });
  window.addEventListener("keydown", preventKeyboardZoom);
  window.addEventListener("gesturestart", preventGestureZoom);
  window.addEventListener("gesturechange", preventGestureZoom);
}
