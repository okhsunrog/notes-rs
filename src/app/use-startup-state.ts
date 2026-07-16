import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getStartupStatus } from "@/lib/api";

export function useStartupState() {
  const [ready, setReady] = useState(false);
  const [startupError, setStartupError] = useState("");

  useEffect(() => {
    let cancelled = false;
    getStartupStatus()
      .then((result) => {
        if (cancelled) return;
        if (result.state === "ready") setReady(true);
        if (result.state === "error") setStartupError(result.message);
      })
      .catch(() => {
        // Startup state may not be registered during the first setup tick.
      });
    const readyListener = listen("app:ready", () => {
      if (!cancelled) setReady(true);
    });
    const errorListener = listen<string>("app:startup-error", ({ payload }) => {
      if (!cancelled) setStartupError(payload);
    });
    return () => {
      cancelled = true;
      void readyListener.then((unlisten) => unlisten());
      void errorListener.then((unlisten) => unlisten());
    };
  }, []);

  return { ready, startupError };
}
