import { useEffect, useState } from "react";
import { getStartupStatus } from "@/lib/api";
import { events } from "@/lib/bindings";

export function useStartupState() {
  const [ready, setReady] = useState(false);
  const [startupError, setStartupError] = useState("");

  useEffect(() => {
    let cancelled = false;
    const readyListener = events.appReady.listen(() => {
      if (!cancelled) setReady(true);
    });
    const errorListener = events.appStartupError.listen(({ payload }) => {
      if (!cancelled) setStartupError(payload.message);
    });

    void getStartupStatus()
      .then((result) => {
        if (cancelled) return;
        if (result.state === "ready") setReady(true);
        if (result.state === "error") setStartupError(result.message);
      })
      .catch(() => {
        // Startup state may not be registered during the first setup tick.
      });
    return () => {
      cancelled = true;
      void readyListener.then((unlisten) => unlisten());
      void errorListener.then((unlisten) => unlisten());
    };
  }, []);

  return { ready, startupError };
}
