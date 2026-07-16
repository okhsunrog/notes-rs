import { createContext, useContext, useEffect, useMemo, useState } from "react";

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
};

const AppearanceContext = createContext<AppearanceContextValue | null>(null);
const STORAGE_KEY = "notes-rs.palette";

export function AppearanceProvider({ children }: { children: React.ReactNode }) {
  const [palette, setPalette] = useState<PaletteId>(() => {
    const saved = localStorage.getItem(STORAGE_KEY);
    return PALETTES.some((item) => item.id === saved) ? (saved as PaletteId) : "iris";
  });

  useEffect(() => {
    document.documentElement.dataset.palette = palette;
    localStorage.setItem(STORAGE_KEY, palette);
  }, [palette]);

  const value = useMemo(() => ({ palette, setPalette }), [palette]);
  return <AppearanceContext.Provider value={value}>{children}</AppearanceContext.Provider>;
}

export function useAppearance() {
  const context = useContext(AppearanceContext);
  if (!context) throw new Error("useAppearance must be used inside AppearanceProvider");
  return context;
}
