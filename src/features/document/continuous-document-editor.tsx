import { defaultKeymap, history, historyKeymap, redo, undo } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { defaultHighlightStyle, syntaxHighlighting } from "@codemirror/language";
import {
  Annotation,
  ChangeSet,
  Compartment,
  EditorState,
  Prec,
  Transaction,
} from "@codemirror/state";
import { drawSelection, EditorView, keymap } from "@codemirror/view";
import { useEffect, useRef } from "react";
import { useResolvedDisplay } from "@/app/appearance";
import type { MarkdownOpenHandler } from "@/features/markdown";
import { documentAuthoringExtensions, type DocumentAuthoringMode } from "./document-live-preview";
import { resolveDocumentHistoryKey } from "./document-editor-model";
import type { DocumentHistoryAction } from "./document-editor-model";
import { resolveDocumentLinkClickDisposition } from "./document-link-navigation";
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

export function minimalExternalDocumentChange(previous: string, next: string): ChangeSet {
  let prefix = 0;
  const sharedLength = Math.min(previous.length, next.length);
  while (prefix < sharedLength && previous[prefix] === next[prefix]) prefix += 1;

  let suffix = 0;
  while (
    suffix < previous.length - prefix &&
    suffix < next.length - prefix &&
    previous[previous.length - suffix - 1] === next[next.length - suffix - 1]
  ) {
    suffix += 1;
  }

  return ChangeSet.of(
    {
      from: prefix,
      to: previous.length - suffix,
      insert: next.slice(prefix, next.length - suffix),
    },
    previous.length,
  );
}

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

/**
 * A blinking caret repaints the panel twice a second on e-ink, so the caret is drawn by
 * CodeMirror there and never blinks. Elsewhere the native caret stays untouched.
 */
function cursorExtensions(display: "standard" | "eink") {
  return display === "eink" ? [drawSelection({ cursorBlinkRate: 0 })] : [];
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
  const display = useResolvedDisplay();
  const mountRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const initialValueRef = useRef(value);
  const callbacksRef = useRef({ onChange, onCompositionEnd, onBlur, onOpenMarkdownLink, pageUuid });
  const modeCompartmentRef = useRef(new Compartment());
  const editableCompartmentRef = useRef(new Compartment());
  const cursorCompartmentRef = useRef(new Compartment());
  const displayRef = useRef(display);
  displayRef.current = display;
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
    const cursorCompartment = cursorCompartmentRef.current;
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
        cursorCompartment.of(cursorExtensions(displayRef.current)),
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
            // Resolved on mousedown, before CodeMirror's own default handling would place the
            // caret and retroactively reveal the link's raw source — the decoration state must
            // reflect what the user actually saw when they clicked, not what it becomes after.
            mousedown(event, view) {
              const { onOpenMarkdownLink, pageUuid: contextPageUuid } = callbacksRef.current;
              if (!onOpenMarkdownLink || !contextPageUuid) return false;
              const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
              if (pos === null) return false;
              const resolved = resolveDocumentLinkClickDisposition(
                view.state,
                pos,
                {
                  button: event.button,
                  ctrlKey: event.ctrlKey,
                  metaKey: event.metaKey,
                  shiftKey: event.shiftKey,
                },
                pendingModeRef.current,
              );
              if (!resolved) return false;
              event.preventDefault();
              void onOpenMarkdownLink({
                context: { kind: "note", presentation: "live_preview", pageUuid: contextPageUuid },
                disposition: resolved.disposition,
                target: resolved.target,
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
    const changes = minimalExternalDocumentChange(view.state.doc.toString(), value);
    const selection = view.state.selection.map(changes);
    view.dispatch({
      changes,
      selection,
      annotations: [externalDocumentUpdate.of(true), Transaction.addToHistory.of(false)],
    });
  }, [value]);

  useEffect(() => {
    viewRef.current?.dispatch({
      effects: editableCompartmentRef.current.reconfigure(editableExtensions(readOnly)),
    });
  }, [readOnly]);

  useEffect(() => {
    viewRef.current?.dispatch({
      effects: cursorCompartmentRef.current.reconfigure(cursorExtensions(display)),
    });
  }, [display]);

  useEffect(() => {
    applyPendingModeRef.current();
  }, [mode]);

  useEffect(() => {
    if (focusRequest > 0 && !readOnly) viewRef.current?.focus();
  }, [focusRequest, readOnly]);

  return <div ref={mountRef} className="continuous-document-editor min-h-96 w-full" />;
}
