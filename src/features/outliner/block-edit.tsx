import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { defaultHighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorState, Prec } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";
import {
  replaceEditorRange,
  type BlockEditKeyEvent,
  type BlockEditPasteEvent,
} from "./block-edit-model";

export type BlockEditHandle = {
  /** Replace text and selection in one CodeMirror transaction. */
  replaceRange: (start: number, end: number, replacement: string, caret?: number) => void;
  focus: () => void;
  getCaret: () => number;
};

type Props = {
  initial: string;
  onChange: (value: string, caret: number) => void;
  onBlur: () => void;
  /** Return true when the outliner handled the key and CM must not. */
  onKeyDown?: (event: BlockEditKeyEvent) => boolean;
  /** Return true when the outliner handled the paste and CM must not. */
  onPaste?: (event: BlockEditPasteEvent) => boolean;
  autoFocus?: boolean;
};

const editorTheme = EditorView.theme({
  "&": {
    width: "100%",
    minHeight: "1.5rem",
    color: "var(--foreground)",
    backgroundColor: "transparent",
    fontSize: "0.875rem",
  },
  "&.cm-focused": {
    outline: "none",
  },
  ".cm-scroller": {
    overflow: "visible",
    fontFamily: "inherit",
    lineHeight: "1.625",
  },
  ".cm-content": {
    minHeight: "1.5rem",
    padding: "0",
    caretColor: "var(--primary)",
    fontFamily: "inherit",
  },
  ".cm-line": {
    padding: "0",
  },
  ".cm-cursor, .cm-dropCursor": {
    borderLeftColor: "var(--primary)",
  },
  ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
    backgroundColor: "color-mix(in oklab, var(--primary) 20%, transparent)",
  },
  ".cm-gutters": {
    display: "none",
  },
});

export const BlockEdit = forwardRef<BlockEditHandle, Props>(function BlockEdit(
  { initial, onChange, onBlur, onKeyDown, onPaste, autoFocus },
  ref,
) {
  const mountRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const initialRef = useRef(initial);
  const autoFocusRef = useRef(autoFocus);
  const callbacksRef = useRef({ onChange, onBlur, onKeyDown, onPaste });
  callbacksRef.current = { onChange, onBlur, onKeyDown, onPaste };

  useImperativeHandle(
    ref,
    () => ({
      replaceRange(start, end, replacement, caret) {
        const view = viewRef.current;
        if (!view) return;
        view.dispatch(replaceEditorRange(view.state, start, end, replacement, caret));
        view.focus();
      },
      focus() {
        viewRef.current?.focus();
      },
      getCaret() {
        return viewRef.current?.state.selection.main.from ?? 0;
      },
    }),
    [],
  );

  useEffect(() => {
    const parent = mountRef.current;
    if (!parent) return;

    const state = EditorState.create({
      doc: initialRef.current,
      selection: autoFocusRef.current ? { anchor: initialRef.current.length } : undefined,
      extensions: [
        markdown(),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        history(),
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({
          "aria-label": "Block content",
          autocapitalize: "sentences",
          autocomplete: "off",
          autocorrect: "on",
          spellcheck: "true",
        }),
        Prec.highest(
          EditorView.domEventHandlers({
            keydown(event, view) {
              const selection = view.state.selection.main;
              const handled =
                callbacksRef.current.onKeyDown?.({
                  key: event.key,
                  altKey: event.altKey,
                  ctrlKey: event.ctrlKey,
                  metaKey: event.metaKey,
                  shiftKey: event.shiftKey,
                  isComposing: event.isComposing || view.composing,
                  selectionStart: selection.from,
                  selectionEnd: selection.to,
                  value: view.state.doc.toString(),
                }) ?? false;
              if (handled) event.preventDefault();
              return handled;
            },
            paste(event, view) {
              const text = event.clipboardData?.getData("text/plain") ?? "";
              if (!text) return false;
              const selection = view.state.selection.main;
              const handled =
                callbacksRef.current.onPaste?.({
                  text,
                  isComposing: view.composing,
                  selectionStart: selection.from,
                  selectionEnd: selection.to,
                }) ?? false;
              if (handled) event.preventDefault();
              return handled;
            },
            blur() {
              callbacksRef.current.onBlur();
              return false;
            },
          }),
        ),
        EditorView.updateListener.of((update) => {
          if (!update.docChanged && !update.selectionSet) return;
          callbacksRef.current.onChange(
            update.state.doc.toString(),
            update.state.selection.main.from,
          );
        }),
        keymap.of([...defaultKeymap, ...historyKeymap]),
        editorTheme,
      ],
    });

    const view = new EditorView({ state, parent });
    viewRef.current = view;
    if (autoFocusRef.current) view.focus();

    return () => {
      viewRef.current = null;
      view.destroy();
    };
  }, []);

  return <div ref={mountRef} className="min-h-6 w-full" />;
});
