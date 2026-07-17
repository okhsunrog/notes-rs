import { defaultKeymap, history, historyKeymap, redo, undo } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { defaultHighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { Annotation, Compartment, EditorState, Prec, Transaction } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { useEffect, useRef } from "react";
import type { MarkdownOpenHandler } from "@/features/markdown";
import { documentAuthoringExtensions, type DocumentAuthoringMode } from "./document-live-preview";
import { resolveDocumentHistoryKey } from "./document-editor-model";
import type { DocumentHistoryAction } from "./document-editor-model";
import {
  resolveDocumentLinkOpenDisposition,
  resolveDocumentLinkTarget,
} from "./document-link-navigation";
import { notesLinkMarkdownExtension } from "./notes-link-markdown-extension";

export type { DocumentAuthoringMode } from "./document-live-preview";

type Props = {
  value: string;
  readOnly: boolean;
  focusRequest: number;
  mode?: DocumentAuthoringMode;
  pageUuid?: string;
  onChange: (value: string, composing: boolean) => void;
  onCompositionEnd: (value: string) => void;
  onBlur: () => void;
  onOpenMarkdownLink?: MarkdownOpenHandler;
};

const externalDocumentUpdate = Annotation.define<boolean>();
const MODE_RETRY_DELAYS_MS = [0, 16, 32, 64, 128, 256] as const;

const documentEditorTheme = EditorView.theme({
  "&": {
    width: "100%",
    minHeight: "24rem",
    color: "var(--foreground)",
    backgroundColor: "transparent",
    fontSize: "0.95rem",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    overflow: "visible",
    fontFamily: "inherit",
    lineHeight: "1.75",
  },
  ".cm-content": {
    minHeight: "24rem",
    padding: "0.25rem 0 4rem",
    caretColor: "var(--primary)",
    fontFamily: "inherit",
  },
  ".cm-line": { padding: "0" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--primary)" },
  ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
    backgroundColor: "color-mix(in oklab, var(--primary) 20%, transparent)",
  },
  ".cm-gutters": { display: "none" },
});

function editableExtensions(readOnly: boolean) {
  return [EditorView.editable.of(!readOnly), EditorState.readOnly.of(readOnly)];
}

/** Always consumes local history keys; read-only panes never mutate their CM state. */
export function applyDocumentHistoryAction(
  view: EditorView,
  action: DocumentHistoryAction,
): boolean {
  if (!view.state.readOnly) {
    if (action === "undo") undo(view);
    else redo(view);
  }
  return true;
}

export function ContinuousDocumentEditor({
  value,
  readOnly,
  focusRequest,
  mode = "live_preview",
  pageUuid,
  onChange,
  onCompositionEnd,
  onBlur,
  onOpenMarkdownLink,
}: Props) {
  const mountRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const initialValueRef = useRef(value);
  const callbacksRef = useRef({ onChange, onCompositionEnd, onBlur, onOpenMarkdownLink, pageUuid });
  const modeCompartmentRef = useRef(new Compartment());
  const editableCompartmentRef = useRef(new Compartment());
  const pendingModeRef = useRef(mode);
  const appliedModeRef = useRef(mode);
  const applyPendingModeRef = useRef<() => void>(() => undefined);
  callbacksRef.current = { onChange, onCompositionEnd, onBlur, onOpenMarkdownLink, pageUuid };
  pendingModeRef.current = mode;

  useEffect(() => {
    const parent = mountRef.current;
    if (!parent) return;
    const modeCompartment = modeCompartmentRef.current;
    const editableCompartment = editableCompartmentRef.current;
    let view: EditorView;
    let destroyed = false;
    let modeRetryTimer: ReturnType<typeof setTimeout> | null = null;
    let modeRetryAttempt = 0;
    const clearModeRetry = () => {
      if (modeRetryTimer !== null) clearTimeout(modeRetryTimer);
      modeRetryTimer = null;
    };
    const applyPendingMode = () => {
      if (destroyed || viewRef.current !== view) return true;
      const pendingMode = pendingModeRef.current;
      if (appliedModeRef.current === pendingMode) return true;
      if (view.composing || view.compositionStarted) return false;
      clearModeRetry();
      view.dispatch({
        effects: modeCompartment.reconfigure(documentAuthoringExtensions(pendingMode)),
      });
      appliedModeRef.current = pendingMode;
      return true;
    };
    const applyModeAfterComposition = () => {
      modeRetryTimer = null;
      if (applyPendingMode()) return;
      const delay = MODE_RETRY_DELAYS_MS[modeRetryAttempt];
      if (delay === undefined) return;
      modeRetryAttempt += 1;
      modeRetryTimer = setTimeout(applyModeAfterComposition, delay);
    };
    const scheduleModeAfterComposition = () => {
      clearModeRetry();
      modeRetryAttempt = 0;
      modeRetryTimer = setTimeout(applyModeAfterComposition, 0);
    };
    const state = EditorState.create({
      doc: initialValueRef.current,
      extensions: [
        markdown({
          base: markdownLanguage,
          extensions: notesLinkMarkdownExtension,
        }),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        history(),
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({
          "aria-label": "Document Markdown",
          autocapitalize: "sentences",
          autocomplete: "off",
          autocorrect: "on",
          spellcheck: "true",
        }),
        modeCompartment.of(documentAuthoringExtensions(mode)),
        editableCompartment.of(editableExtensions(readOnly)),
        Prec.highest(
          EditorView.domEventHandlers({
            keydown(event, view) {
              const action = resolveDocumentHistoryKey({
                key: event.key,
                ctrlKey: event.ctrlKey,
                metaKey: event.metaKey,
                shiftKey: event.shiftKey,
                altKey: event.altKey,
                isComposing: event.isComposing || view.composing,
              });
              if (!action) return false;
              applyDocumentHistoryAction(view, action);
              event.preventDefault();
              event.stopPropagation();
              return true;
            },
            compositionend(_event, view) {
              queueMicrotask(() => {
                callbacksRef.current.onCompositionEnd(view.state.doc.toString());
              });
              scheduleModeAfterComposition();
              return false;
            },
            blur() {
              callbacksRef.current.onBlur();
              scheduleModeAfterComposition();
              return false;
            },
            click(event, view) {
              const { onOpenMarkdownLink, pageUuid: contextPageUuid } = callbacksRef.current;
              if (!onOpenMarkdownLink || !contextPageUuid) return false;
              const disposition = resolveDocumentLinkOpenDisposition({
                button: event.button,
                ctrlKey: event.ctrlKey,
                metaKey: event.metaKey,
                shiftKey: event.shiftKey,
              });
              if (!disposition) return false;
              if (!(event.target instanceof Node)) return false;
              const pos = view.posAtDOM(event.target);
              const target = resolveDocumentLinkTarget(view.state, pos);
              if (!target) return false;
              event.preventDefault();
              void onOpenMarkdownLink({
                context: { kind: "note", presentation: "live_preview", pageUuid: contextPageUuid },
                disposition,
                target,
              });
              return true;
            },
          }),
        ),
        EditorView.updateListener.of((update) => {
          if (
            appliedModeRef.current !== pendingModeRef.current &&
            !update.view.composing &&
            !update.view.compositionStarted
          ) {
            queueMicrotask(applyPendingMode);
          }
          if (!update.docChanged) return;
          if (
            update.transactions.some((transaction) =>
              transaction.annotation(externalDocumentUpdate),
            )
          ) {
            return;
          }
          callbacksRef.current.onChange(update.state.doc.toString(), update.view.composing);
        }),
        keymap.of([...defaultKeymap, ...historyKeymap]),
        documentEditorTheme,
      ],
    });
    view = new EditorView({ state, parent });
    viewRef.current = view;
    applyPendingModeRef.current = applyPendingMode;
    return () => {
      destroyed = true;
      clearModeRetry();
      applyPendingModeRef.current = () => undefined;
      viewRef.current = null;
      view.destroy();
    };
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || view.composing || view.state.doc.toString() === value) return;
    const anchor = Math.min(view.state.selection.main.anchor, value.length);
    const head = Math.min(view.state.selection.main.head, value.length);
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: value },
      selection: { anchor, head },
      annotations: [externalDocumentUpdate.of(true), Transaction.addToHistory.of(false)],
    });
  }, [value]);

  useEffect(() => {
    viewRef.current?.dispatch({
      effects: editableCompartmentRef.current.reconfigure(editableExtensions(readOnly)),
    });
  }, [readOnly]);

  useEffect(() => {
    applyPendingModeRef.current();
  }, [mode]);

  useEffect(() => {
    if (focusRequest > 0 && !readOnly) viewRef.current?.focus();
  }, [focusRequest, readOnly]);

  return <div ref={mountRef} className="continuous-document-editor min-h-96 w-full" />;
}
