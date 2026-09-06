import { createContext, useContext, useEffect, useMemo, useState } from "react";
import { useTheme } from "next-themes";
import { getMobileSystemInfo, setSystemBarsStyle } from "@/lib/api";
import {
  DISPLAY_PROFILE_STORAGE_KEY,
  INK_COLOR_STORAGE_KEY,
  parseDisplayProfile,
  parseInkColor,
  resolveDisplay,
  type DisplayInfo,
  type DisplayProfile,
  type InkColorPreference,
  type ResolvedDisplay,
  type ResolvedInkColor,
} from "./display-profile";

export const PALETTES = [
  {
    id: "iris",
    name: "Iris",
    description: "Violet, periwinkle, and soft ink",
    swatches: ["#7767df", "#b7a9ff", "#ebe8ff"],
  },
  {
    id: "tidal",
    name: "Tidal",
    description: "Deep teal and clear cyan",
    swatches: ["#078b91", "#57c7c9", "#d8f3f2"],
  },
  {
    id: "ember",
    name: "Ember",
    description: "Warm amber and burnt orange",
    swatches: ["#dc6b28", "#f3a65a", "#fff0dc"],
  },
  {
    id: "sakura",
    name: "Sakura",
    description: "Rose, plum, and blush",
    swatches: ["#c64c7b", "#ee8db2", "#ffe5ef"],
  },
  {
    id: "nordic",
    name: "Nordic",
    description: "Glacier blue and cool slate",
    swatches: ["#3979d1", "#83b6ed", "#e2effc"],
  },
  {
    id: "moss",
    name: "Moss",
    description: "Botanical green and sage",
    swatches: ["#42865c", "#86ba8c", "#e4f2df"],
  },
] as const;

export type PaletteId = (typeof PALETTES)[number]["id"];

type AppearanceContextValue = {
  palette: PaletteId;
  setPalette: (palette: PaletteId) => void;
  displayProfile: DisplayProfile;
  setDisplayProfile: (profile: DisplayProfile) => void;
  inkColor: InkColorPreference;
  setInkColor: (color: InkColorPreference) => void;
  /** The profile actually in effect, after "auto" was resolved against the reported panel. */
  display: ResolvedDisplay;
  /** The ink color actually in effect. Components read this instead of the DOM. */
  resolvedInkColor: ResolvedInkColor;
};

const AppearanceContext = createContext<AppearanceContextValue | null>(null);
const STORAGE_KEY = "tangleaf.palette";

export function AppearanceProvider({ children }: { children: React.ReactNode }) {
  const { resolvedTheme } = useTheme();
  const [palette, setPalette] = useState<PaletteId>(() => {
    const saved = localStorage.getItem(STORAGE_KEY);
    return PALETTES.some((item) => item.id === saved) ? (saved as PaletteId) : "iris";
  });
  const [displayProfile, setDisplayProfile] = useState<DisplayProfile>(() =>
    parseDisplayProfile(localStorage.getItem(DISPLAY_PROFILE_STORAGE_KEY)),
  );
  const [inkColor, setInkColor] = useState<InkColorPreference>(() =>
    parseInkColor(localStorage.getItem(INK_COLOR_STORAGE_KEY)),
  );
  const [displayInfo, setDisplayInfo] = useState<DisplayInfo>(null);
  const resolved = useMemo(
    () => resolveDisplay({ profile: displayProfile, inkColor, info: displayInfo }),
    [displayProfile, inkColor, displayInfo],
  );

  useEffect(() => {
    document.documentElement.dataset.palette = palette;
    localStorage.setItem(STORAGE_KEY, palette);
  }, [palette]);

  useEffect(() => {
    localStorage.setItem(DISPLAY_PROFILE_STORAGE_KEY, displayProfile);
  }, [displayProfile]);

  useEffect(() => {
    localStorage.setItem(INK_COLOR_STORAGE_KEY, inkColor);
  }, [inkColor]);

  // The single writer of the display dataset attributes every stylesheet variant keys off.
  useEffect(() => {
    document.documentElement.dataset.display = resolved.display;
    document.documentElement.dataset.inkColor = resolved.color;
  }, [resolved]);

  useEffect(() => {
    const root = document.documentElement;
    const updateInsets = async () => {
      const info = await getMobileSystemInfo();
      if (!info) return;
      root.dataset.mobile = "true";
      setDisplayInfo((previous) =>
        previous?.displayKind === info.displayKind && previous.colorPanel === info.colorPanel
          ? previous
          : { displayKind: info.displayKind, colorPanel: info.colorPanel },
      );
      root.style.setProperty("--safe-area-inset-top", `${info.safeArea.top}px`);
      root.style.setProperty("--safe-area-inset-right", `${info.safeArea.right}px`);
      root.style.setProperty("--safe-area-inset-bottom", `${info.safeArea.bottom}px`);
      root.style.setProperty("--safe-area-inset-left", `${info.safeArea.left}px`);
    };

    void updateInsets().catch(() => undefined);
    window.addEventListener("resize", updateInsets);
    window.addEventListener("orientationchange", updateInsets);
    return () => {
      window.removeEventListener("resize", updateInsets);
      window.removeEventListener("orientationchange", updateInsets);
    };
  }, []);

  useEffect(() => {
    if (!resolvedTheme) return;
    void setSystemBarsStyle(resolvedTheme === "dark").catch(() => undefined);
  }, [resolvedTheme]);

  const value = useMemo(
    () => ({
      palette,
      setPalette,
      displayProfile,
      setDisplayProfile,
      inkColor,
      setInkColor,
      display: resolved.display,
      resolvedInkColor: resolved.color,
    }),
    [palette, displayProfile, inkColor, resolved],
  );
  return <AppearanceContext.Provider value={value}>{children}</AppearanceContext.Provider>;
}

export function useAppearance() {
  const context = useContext(AppearanceContext);
  if (!context) throw new Error("useAppearance must be used inside AppearanceProvider");
  return context;
}
