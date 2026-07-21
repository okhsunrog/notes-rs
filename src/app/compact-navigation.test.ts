import { describe, expect, it } from "vitest";
import {
  CompactRegion,
  compactNavigationReducer,
  initialCompactNavigation,
  workbenchIsVisible,
} from "./compact-navigation";

describe("compact navigation", () => {
  it("opens pushed content in the workbench", () => {
    const state = compactNavigationReducer(initialCompactNavigation, { type: "open_editor" });

    expect(state.region).toBe(CompactRegion.Editor);
    expect(workbenchIsVisible(state.region)).toBe(true);
  });

  it("returns from Assistant to the surface that opened it", () => {
    const editor = compactNavigationReducer(initialCompactNavigation, { type: "open_editor" });
    const assistant = compactNavigationReducer(editor, { type: "open_assistant" });

    expect(compactNavigationReducer(assistant, { type: "close_assistant" })).toEqual(editor);
  });

  it("uses the workbench for both the Dashboard root and pushed content", () => {
    expect(workbenchIsVisible(CompactRegion.Home)).toBe(true);
    expect(workbenchIsVisible(CompactRegion.Editor)).toBe(true);
    expect(workbenchIsVisible(CompactRegion.Assistant)).toBe(false);
  });
});
