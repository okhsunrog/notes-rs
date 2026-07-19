import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import {
  replaceEditorRange,
  resolveBlockEditKey,
  runStructuralEditAfterFlush,
  splitEditorContent,
  type BlockEditKeyEvent,
} from "./block-edit-model";

function key(keyValue: string, overrides: Partial<BlockEditKeyEvent> = {}): BlockEditKeyEvent {
  return {
    key: keyValue,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    isComposing: false,
    selectionStart: 2,
    selectionEnd: 2,
    value: "draft",
    ...overrides,
  };
}

const idleContext = {
  autocompleteOpen: false,
  autocompleteHasItems: false,
  draftEmpty: false,
};

describe("active block editor model", () => {
  it("replaces a range and moves the selection in one transaction", () => {
    const state = EditorState.create({ doc: "before [[pa after" });
    const transaction = replaceEditorRange(state, 7, 11, "[[Page]]");

    expect(transaction.state.doc.toString()).toBe("before [[Page]] after");
    expect(transaction.state.selection.main.anchor).toBe(15);
    expect(transaction.isUserEvent("input.complete")).toBe(true);
  });

  it("clamps an external replacement and explicit caret to the resulting document", () => {
    const state = EditorState.create({ doc: "abc" });
    const transaction = replaceEditorRange(state, -10, 20, "x", 99);

    expect(transaction.state.doc.toString()).toBe("x");
    expect(transaction.state.selection.main.anchor).toBe(1);
  });

  it("never turns a composing key into an outliner command", () => {
    const resolution = resolveBlockEditKey(key("Enter", { isComposing: true }), idleContext);

    expect(resolution).toEqual({ action: "native", closeAutocomplete: false });
  });

  it("keeps Shift+Enter native but maps plain Enter to a block split", () => {
    expect(resolveBlockEditKey(key("Enter", { shiftKey: true }), idleContext).action).toBe(
      "native",
    );
    expect(resolveBlockEditKey(key("Enter"), idleContext).action).toBe("split");
  });

  it("splits plain Enter at the actual selection and removes the selected range", () => {
    expect(splitEditorContent("left selected right", 5, 13)).toEqual(["left ", " right"]);
    expect(splitEditorContent("abcdef", 4, 2)).toEqual(["ab", "ef"]);
  });

  it("flushes immediate typing before Enter splits against the acknowledged revision", async () => {
    let revision = "r1";
    const events: string[] = [];

    const completed = await runStructuralEditAfterFlush(
      async () => {
        events.push("save typed draft at r1");
        await Promise.resolve();
        revision = "r2";
        return true;
      },
      async () => {
        events.push(`split at ${revision}`);
        if (revision !== "r2") throw new Error("conflict");
      },
    );

    expect(completed).toBe(true);
    expect(events).toEqual(["save typed draft at r1", "split at r2"]);
  });

  it("gives an open autocomplete menu priority over structural commands", () => {
    const context = {
      autocompleteOpen: true,
      autocompleteHasItems: true,
      draftEmpty: false,
    };

    expect(resolveBlockEditKey(key("ArrowDown"), context).action).toBe("next-autocomplete");
    expect(resolveBlockEditKey(key("Tab"), context).action).toBe("accept-autocomplete");
  });

  it("dismisses an empty autocomplete before applying ordinary Enter behavior", () => {
    const resolution = resolveBlockEditKey(key("Enter"), {
      autocompleteOpen: true,
      autocompleteHasItems: false,
      draftEmpty: false,
    });

    expect(resolution).toEqual({ action: "split", closeAutocomplete: true });
  });

  it("maps boundary navigation and indentation without inspecting a DOM element", () => {
    expect(resolveBlockEditKey(key("ArrowUp", { selectionStart: 0 }), idleContext).action).toBe(
      "move-previous",
    );
    expect(resolveBlockEditKey(key("ArrowDown", { selectionEnd: 5 }), idleContext).action).toBe(
      "move-next",
    );
    expect(resolveBlockEditKey(key("Tab"), idleContext).action).toBe("indent");
    expect(resolveBlockEditKey(key("Tab", { shiftKey: true }), idleContext).action).toBe("outdent");
  });
});
