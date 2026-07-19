import { EditorState, Transaction } from "@codemirror/state";

export type BlockEditKeyEvent = Readonly<{
  key: string;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  isComposing: boolean;
  selectionStart: number;
  selectionEnd: number;
  value: string;
}>;

export type BlockEditPasteEvent = Readonly<{
  text: string;
  isComposing: boolean;
  selectionStart: number;
  selectionEnd: number;
}>;

export type BlockEditKeyAction =
  | "accept-autocomplete"
  | "close-autocomplete"
  | "collapse"
  | "delete-empty"
  | "indent"
  | "move-next"
  | "move-previous"
  | "native"
  | "next-autocomplete"
  | "outdent"
  | "previous-autocomplete"
  | "reorder-down"
  | "reorder-up"
  | "split"
  | "stop-editing";

export type BlockEditKeyResolution = Readonly<{
  action: BlockEditKeyAction;
  closeAutocomplete: boolean;
}>;

type BlockEditKeyContext = Readonly<{
  autocompleteOpen: boolean;
  autocompleteHasItems: boolean;
  draftEmpty: boolean;
}>;

const nativeKey: BlockEditKeyResolution = {
  action: "native",
  closeAutocomplete: false,
};

/**
 * Resolves structural outliner shortcuts without depending on a textarea,
 * React, or CodeMirror. `native` means the editor should keep handling the
 * keystroke itself (including every keystroke emitted during IME composition).
 */
export function resolveBlockEditKey(
  event: BlockEditKeyEvent,
  context: BlockEditKeyContext,
): BlockEditKeyResolution {
  if (event.isComposing) return nativeKey;

  if (context.autocompleteOpen) {
    if (event.key === "ArrowDown") {
      return { action: "next-autocomplete", closeAutocomplete: false };
    }
    if (event.key === "ArrowUp") {
      return { action: "previous-autocomplete", closeAutocomplete: false };
    }
    if (event.key === "Enter" || event.key === "Tab") {
      if (context.autocompleteHasItems) {
        return { action: "accept-autocomplete", closeAutocomplete: false };
      }
      if (event.key === "Tab") {
        return { action: "close-autocomplete", closeAutocomplete: true };
      }
      // Empty Enter dismisses the menu, then keeps its ordinary split/newline
      // behavior below.
    }
    if (event.key === "Escape") {
      return { action: "close-autocomplete", closeAutocomplete: true };
    }
  }

  const closeAutocomplete =
    context.autocompleteOpen &&
    (event.key === "Enter" || event.key === "Tab" || event.key === "Escape");

  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
    return { action: "collapse", closeAutocomplete };
  }
  if ((event.ctrlKey || event.metaKey) && event.key === "ArrowUp") {
    return { action: "reorder-up", closeAutocomplete };
  }
  if ((event.ctrlKey || event.metaKey) && event.key === "ArrowDown") {
    return { action: "reorder-down", closeAutocomplete };
  }
  if (event.key === "Enter" && !event.shiftKey && !event.ctrlKey && !event.metaKey) {
    return { action: "split", closeAutocomplete };
  }
  if (event.key === "Backspace" && context.draftEmpty) {
    return { action: "delete-empty", closeAutocomplete };
  }
  if (event.key === "Tab") {
    return {
      action: event.shiftKey ? "outdent" : "indent",
      closeAutocomplete,
    };
  }
  if (event.key === "ArrowUp" && event.selectionStart === 0) {
    return { action: "move-previous", closeAutocomplete };
  }
  if (event.key === "ArrowDown" && event.selectionEnd === event.value.length) {
    return { action: "move-next", closeAutocomplete };
  }
  if (event.key === "Escape") {
    return { action: "stop-editing", closeAutocomplete };
  }
  return { action: "native", closeAutocomplete };
}

function clampOffset(offset: number, length: number) {
  return Math.max(0, Math.min(offset, length));
}

/** Splits around the actual CM selection, dropping selected text like the old textarea editor. */
export function splitEditorContent(
  value: string,
  selectionStart: number,
  selectionEnd: number,
): readonly [string, string] {
  const from = clampOffset(Math.min(selectionStart, selectionEnd), value.length);
  const to = clampOffset(Math.max(selectionStart, selectionEnd), value.length);
  return [value.slice(0, from), value.slice(to)];
}

export async function runStructuralEditAfterFlush(
  flush: () => Promise<boolean>,
  edit: () => Promise<void>,
): Promise<boolean> {
  if (!(await flush())) return false;
  await edit();
  return true;
}

/** Builds the single transaction used by autocomplete and other host inserts. */
export function replaceEditorRange(
  state: EditorState,
  start: number,
  end: number,
  replacement: string,
  caret?: number,
): Transaction {
  const from = clampOffset(Math.min(start, end), state.doc.length);
  const to = clampOffset(Math.max(start, end), state.doc.length);
  const nextLength = state.doc.length - (to - from) + replacement.length;
  const anchor = clampOffset(caret ?? from + replacement.length, nextLength);

  return state.update({
    changes: { from, to, insert: replacement },
    selection: { anchor },
    scrollIntoView: true,
    annotations: Transaction.userEvent.of("input.complete"),
  });
}
