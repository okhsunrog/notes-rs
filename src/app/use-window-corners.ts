import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

export function useWindowCorners(enabled: boolean, radius: number) {
  useEffect(() => {
    if (!enabled) return;
    const root = document.documentElement;
    const appWindow = getCurrentWindow();
    let disposed = false;
    let revision = 0;
    const update = async () => {
      const current = ++revision;
      const [maximized, fullscreen] = await Promise.all([
        appWindow.isMaximized(),
        appWindow.isFullscreen(),
      ]);
      if (disposed || current !== revision) return;
      root.dataset.roundedWindow = "true";
      root.style.setProperty("--window-corner-radius", `${maximized || fullscreen ? 0 : radius}px`);
    };
    const refresh = () => {
      void update().catch(console.error);
    };
    const listener = appWindow.onResized(refresh);
    refresh();
    return () => {
      disposed = true;
      void listener.then((unlisten) => unlisten()).catch(console.error);
      delete root.dataset.roundedWindow;
      root.style.removeProperty("--window-corner-radius");
    };
  }, [enabled, radius]);
}
