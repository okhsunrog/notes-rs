import { createContext, useContext, useEffect, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { addPluginListener } from "@tauri-apps/api/core";
import { create } from "zustand";
import { persist } from "zustand/middleware";
import { getInputCapabilities } from "@/lib/api";
import type { InputCapabilities } from "@/lib/bindings";
import { queryKeys } from "@/lib/query";

export type HandwritingPreference = "auto" | "always" | "hidden";

export function handwritingAvailable(
  preference: HandwritingPreference,
  stylus: InputCapabilities["stylus"],
  observedPen: boolean,
) {
  if (preference === "hidden") return false;
  return preference === "always" || stylus === "available" || observedPen;
}

export const useHandwritingPreference = create(
  persist<{
    mode: HandwritingPreference;
    mouseEnabled: boolean;
    setMouseEnabled: (mouseEnabled: boolean) => void;
    setMode: (mode: HandwritingPreference) => void;
  }>(
    (set) => ({
      mode: "auto",
      mouseEnabled: false,
      setMouseEnabled: (mouseEnabled) => set({ mouseEnabled }),
      setMode: (mode) => set({ mode }),
    }),
    {
      name: "tangleaf.handwriting-preference.v1",
    },
  ),
);

const unknown: InputCapabilities = {
  stylus: "unknown",
  pressure: false,
  tilt: false,
  nativeDeviceEvents: false,
};
const InputContext = createContext({ capabilities: unknown, observedPen: false });
const useObservedPen = create<{ observed: boolean }>(() => ({ observed: false }));

export function InputCapabilitiesProvider({ children }: { children: ReactNode }) {
  const client = useQueryClient();
  const query = useQuery({ queryKey: queryKeys.inputCapabilities, queryFn: getInputCapabilities });
  const capabilities = query.data ?? unknown;
  const observedPen = useObservedPen((state) => state.observed);

  useEffect(() => {
    const refresh = () => {
      if (document.visibilityState === "hidden") return;
      void client.invalidateQueries({ queryKey: queryKeys.inputCapabilities });
    };
    const observe = (event: PointerEvent) => {
      if (event.pointerType === "pen") useObservedPen.setState({ observed: true });
    };
    document.addEventListener("visibilitychange", refresh);
    window.addEventListener("focus", refresh);
    window.addEventListener("pointerdown", observe, true);
    window.addEventListener("pointerover", observe, true);
    return () => {
      document.removeEventListener("visibilitychange", refresh);
      window.removeEventListener("focus", refresh);
      window.removeEventListener("pointerdown", observe, true);
      window.removeEventListener("pointerover", observe, true);
    };
  }, [client]);

  useEffect(() => {
    if (!capabilities.nativeDeviceEvents) return;
    let disposed = false;
    let remove: (() => Promise<void>) | undefined;
    void addPluginListener("mobile-system", "inputDevicesChanged", () => {
      useObservedPen.setState({ observed: false });
      void client.invalidateQueries({ queryKey: queryKeys.inputCapabilities });
    })
      .then((listener) => {
        if (disposed) void listener.unregister();
        else {
          remove = () => listener.unregister();
          // Close the gap between the initial snapshot and listener registration.
          void client.invalidateQueries({ queryKey: queryKeys.inputCapabilities });
        }
      })
      .catch((error: unknown) => console.error("Input device listener failed", error));
    return () => {
      disposed = true;
      void remove?.();
    };
  }, [capabilities.nativeDeviceEvents, client]);

  return <InputContext value={{ capabilities, observedPen }}>{children}</InputContext>;
}

export function useHandwritingAvailability() {
  const { capabilities, observedPen } = useContext(InputContext);
  const mode = useHandwritingPreference((state) => state.mode);
  return {
    capabilities,
    available: handwritingAvailable(mode, capabilities.stylus, observedPen),
  };
}
