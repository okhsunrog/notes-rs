export class DebouncedAction {
  private timer: ReturnType<typeof setTimeout> | null = null;

  schedule(action: () => void, delayMs: number) {
    this.cancel();
    this.timer = setTimeout(() => {
      this.timer = null;
      action();
    }, delayMs);
  }

  cancel() {
    if (!this.timer) return false;
    clearTimeout(this.timer);
    this.timer = null;
    return true;
  }

  get pending() {
    return this.timer !== null;
  }
}
