import { describe, expect, it } from "vite-plus/test";
import {
  parseDisplayProfile,
  parseInkColor,
  resolveDisplay,
  type DisplayInfo,
} from "./display-profile";

const eink: DisplayInfo = { displayKind: "eink", colorPanel: null };
const lcd: DisplayInfo = { displayKind: "lcd", colorPanel: null };

describe("resolveDisplay", () => {
  it("keeps the standard profile until a panel is known to be e-ink", () => {
    expect(resolveDisplay({ profile: "auto", inkColor: "auto", info: null }).display).toBe(
      "standard",
    );
    expect(resolveDisplay({ profile: "auto", inkColor: "auto", info: lcd }).display).toBe(
      "standard",
    );
    expect(
      resolveDisplay({
        profile: "auto",
        inkColor: "auto",
        info: { displayKind: "unknown", colorPanel: null },
      }).display,
    ).toBe("standard");
    expect(resolveDisplay({ profile: "auto", inkColor: "auto", info: eink }).display).toBe("eink");
  });

  it("lets an explicit profile override what the platform reported", () => {
    expect(resolveDisplay({ profile: "eink", inkColor: "auto", info: lcd }).display).toBe("eink");
    expect(resolveDisplay({ profile: "standard", inkColor: "auto", info: eink }).display).toBe(
      "standard",
    );
  });

  it("only assumes a monochrome panel when the platform says the panel has no color", () => {
    expect(resolveDisplay({ profile: "auto", inkColor: "auto", info: null }).color).toBe("color");
    expect(resolveDisplay({ profile: "auto", inkColor: "auto", info: eink }).color).toBe("color");
    expect(
      resolveDisplay({
        profile: "auto",
        inkColor: "auto",
        info: { displayKind: "eink", colorPanel: true },
      }).color,
    ).toBe("color");
    expect(
      resolveDisplay({
        profile: "auto",
        inkColor: "auto",
        info: { displayKind: "eink", colorPanel: false },
      }).color,
    ).toBe("mono");
  });

  it("lets an explicit ink color override the reported panel", () => {
    expect(
      resolveDisplay({
        profile: "auto",
        inkColor: "color",
        info: { displayKind: "eink", colorPanel: false },
      }).color,
    ).toBe("color");
    expect(resolveDisplay({ profile: "auto", inkColor: "mono", info: null }).color).toBe("mono");
  });
});

describe("stored preference parsing", () => {
  it("falls back to auto for missing or unknown values", () => {
    expect(parseDisplayProfile(null)).toBe("auto");
    expect(parseDisplayProfile("nonsense")).toBe("auto");
    expect(parseDisplayProfile("eink")).toBe("eink");
    expect(parseDisplayProfile("standard")).toBe("standard");
    expect(parseInkColor(null)).toBe("auto");
    expect(parseInkColor("greyscale")).toBe("auto");
    expect(parseInkColor("mono")).toBe("mono");
    expect(parseInkColor("color")).toBe("color");
  });
});
