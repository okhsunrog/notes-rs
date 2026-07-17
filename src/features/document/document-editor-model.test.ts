import { describe, expect, it } from "vite-plus/test";
import { resolveDocumentHistoryKey } from "./document-editor-model";

const key = (
  value: string,
  overrides: Partial<Parameters<typeof resolveDocumentHistoryKey>[0]> = {},
) => ({
  key: value,
  ctrlKey: true,
  metaKey: false,
  shiftKey: false,
  ...overrides,
});

describe("document editor keyboard boundary", () => {
  it("keeps undo and redo inside focused CodeMirror", () => {
    expect(resolveDocumentHistoryKey(key("z"))).toBe("undo");
    expect(resolveDocumentHistoryKey(key("z", { shiftKey: true }))).toBe("redo");
    expect(resolveDocumentHistoryKey(key("y"))).toBe("redo");
    expect(resolveDocumentHistoryKey(key("z", { ctrlKey: false, metaKey: true }))).toBe("undo");
  });

  it("does not intercept history shortcuts during IME composition", () => {
    expect(resolveDocumentHistoryKey(key("z", { isComposing: true }))).toBeNull();
    expect(resolveDocumentHistoryKey(key("z", { altKey: true }))).toBeNull();
    expect(resolveDocumentHistoryKey(key("a"))).toBeNull();
  });
});
