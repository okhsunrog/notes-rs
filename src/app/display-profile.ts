import type { MobileSystemInfo } from "@/lib/bindings";

/** What the user asked for. "auto" defers to the panel the platform reports. */
export type DisplayProfile = "auto" | "standard" | "eink";
export type InkColorPreference = "auto" | "color" | "mono";

/** What the UI actually renders. Every stylesheet rule keys off these two values. */
export type ResolvedDisplay = "standard" | "eink";
export type ResolvedInkColor = "color" | "mono";

export type DisplayResolution = { display: ResolvedDisplay; color: ResolvedInkColor };

/** Only the panel facts matter here, so the tests do not have to build a whole system info. */
export type DisplayInfo = Pick<MobileSystemInfo, "displayKind" | "colorPanel"> | null;

export const DISPLAY_PROFILE_STORAGE_KEY = "tangleaf.display-profile";
export const INK_COLOR_STORAGE_KEY = "tangleaf.ink-color";

const DISPLAY_PROFILES: readonly DisplayProfile[] = ["auto", "standard", "eink"];
const INK_COLORS: readonly InkColorPreference[] = ["auto", "color", "mono"];

export const DISPLAY_PROFILE_OPTIONS: ReadonlyArray<{ value: DisplayProfile; label: string }> = [
  { value: "auto", label: "Auto (detect the panel)" },
  { value: "standard", label: "Standard" },
  { value: "eink", label: "E-ink" },
];

export const INK_COLOR_OPTIONS: ReadonlyArray<{ value: InkColorPreference; label: string }> = [
  { value: "auto", label: "Auto" },
  { value: "color", label: "Color accents" },
  { value: "mono", label: "Monochrome" },
];

export function parseDisplayProfile(value: string | null): DisplayProfile {
  return DISPLAY_PROFILES.find((profile) => profile === value) ?? "auto";
}

export function parseInkColor(value: string | null): InkColorPreference {
  return INK_COLORS.find((color) => color === value) ?? "auto";
}

/**
 * Resolves the two appearance preferences against the panel the platform reported. A device that
 * reports nothing (desktop, or an unknown mobile) keeps the standard profile: e-ink treatment is
 * only ever applied to a panel that is known to be e-ink or to a display the user chose by hand.
 */
export function resolveDisplay({
  profile,
  inkColor,
  info,
}: {
  profile: DisplayProfile;
  inkColor: InkColorPreference;
  info: DisplayInfo;
}): DisplayResolution {
  const display: ResolvedDisplay =
    profile === "auto" ? (info?.displayKind === "eink" ? "eink" : "standard") : profile;
  // A grayscale panel is only assumed when the platform says so; unknown stays colorful.
  const color: ResolvedInkColor =
    inkColor === "auto" ? (info?.colorPanel === false ? "mono" : "color") : inkColor;
  return { display, color };
}
