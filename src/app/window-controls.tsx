import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";

export function WindowControls() {
  const window = getCurrentWindow();
  return (
    <div className="flex items-center" aria-label="Window controls">
      <button
        type="button"
        className="flex size-8 items-center justify-center rounded-sm hover:bg-accent"
        aria-label="Minimize window"
        onClick={() => void window.minimize()}
      >
        <Minus className="size-4" />
      </button>
      <button
        type="button"
        className="flex size-8 items-center justify-center rounded-sm hover:bg-accent"
        aria-label="Maximize or restore window"
        onClick={() => void window.toggleMaximize()}
      >
        <Square className="size-3" />
      </button>
      <button
        type="button"
        className="flex size-8 items-center justify-center rounded-sm hover:bg-destructive hover:text-destructive-foreground"
        aria-label="Close window"
        onClick={() => void window.close()}
      >
        <X className="size-4" />
      </button>
    </div>
  );
}
