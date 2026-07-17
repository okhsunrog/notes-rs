import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension, Range } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";
import {
  NOTES_LINK_CONTAINER_NODE_NAMES,
  notesLinkNodeRole,
} from "./notes-link-markdown-extension";

export type DocumentAuthoringMode = "live_preview" | "source";

type TextRange = Readonly<{ from: number; to: number }>;
type MarkdownSyntaxNode = ReturnType<ReturnType<typeof syntaxTree>["resolveInner"]>;

const VIEWPORT_BUFFER = 2_048;
const HEADING_NODE = /^(?:ATX|Setext)Heading([1-6])$/;
const SOURCE_CONSTRUCTS = new Set([
  "Blockquote",
  "Emphasis",
  "FencedCode",
  "InlineCode",
  "Link",
  "ListItem",
  "SetextHeading1",
  "SetextHeading2",
  "Strikethrough",
  "StrongEmphasis",
  "Task",
  "ATXHeading1",
  "ATXHeading2",
  "ATXHeading3",
  "ATXHeading4",
  "ATXHeading5",
  "ATXHeading6",
  ...NOTES_LINK_CONTAINER_NODE_NAMES,
]);

const hiddenSyntax = Decoration.mark({
  class: "cm-lp-hidden-syntax",
  attributes: {
    "aria-hidden": "true",
    "data-live-preview-hidden": "true",
  },
});
const listMarker = Decoration.mark({
  class: "cm-lp-list-marker",
  attributes: { "data-live-preview-marker": "list" },
});

const MAX_TASK_LABEL_LENGTH = 160;

/** A native, keyboard-operable checkbox that replaces the raw `[ ]`/`[x]` marker outside the caret. */
class TaskCheckboxWidget extends WidgetType {
  constructor(
    private readonly checked: boolean,
    private readonly label: string,
    private readonly markerFrom: number,
    private readonly readOnly: boolean,
  ) {
    super();
  }

  override eq(other: TaskCheckboxWidget): boolean {
    return (
      this.checked === other.checked &&
      this.label === other.label &&
      this.markerFrom === other.markerFrom &&
      this.readOnly === other.readOnly
    );
  }

  override toDOM(view: EditorView): HTMLElement {
    const wrapper = document.createElement("span");
    wrapper.className = "cm-lp-task-checkbox";
    wrapper.setAttribute("data-live-preview-marker", "task");
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = this.checked;
    input.disabled = this.readOnly;
    input.setAttribute(
      "aria-label",
      `Mark "${this.label || "task"}" as ${this.checked ? "not done" : "done"}`,
    );
    input.addEventListener("click", (event) => {
      event.preventDefault();
      if (view.state.readOnly) return;
      const insert = this.checked ? " " : "x";
      view.dispatch({ changes: { from: this.markerFrom + 1, to: this.markerFrom + 2, insert } });
      view.focus();
    });
    wrapper.append(input);
    return wrapper;
  }
}

function taskLabel(
  state: EditorState,
  marker: MarkdownSyntaxNode,
  task: MarkdownSyntaxNode,
): string {
  const raw = state.doc.sliceString(marker.to, task.to).trim();
  return raw.length > MAX_TASK_LABEL_LENGTH ? `${raw.slice(0, MAX_TASK_LABEL_LENGTH)}…` : raw;
}

const semanticMarks = {
  blockquote: Decoration.mark({ class: "cm-lp-blockquote" }),
  emphasis: Decoration.mark({ class: "cm-lp-emphasis" }),
  fencedCode: Decoration.mark({ class: "cm-lp-fenced-code" }),
  inlineCode: Decoration.mark({ class: "cm-lp-inline-code" }),
  link: Decoration.mark({ class: "cm-lp-link" }),
  strike: Decoration.mark({ class: "cm-lp-strike" }),
  strong: Decoration.mark({ class: "cm-lp-strong" }),
} as const;

const headingMarks = Array.from({ length: 6 }, (_, index) =>
  Decoration.mark({ class: `cm-lp-heading cm-lp-heading-${index + 1}` }),
);

function mergeRanges(ranges: readonly TextRange[]): TextRange[] {
  const sorted = [...ranges]
    .filter((range) => range.to > range.from)
    .sort((left, right) => left.from - right.from || left.to - right.to);
  const merged: TextRange[] = [];
  for (const range of sorted) {
    const previous = merged[merged.length - 1];
    if (!previous || range.from > previous.to) {
      merged.push({ ...range });
      continue;
    }
    merged[merged.length - 1] = { from: previous.from, to: Math.max(previous.to, range.to) };
  }
  return merged;
}

function bufferedVisibleRanges(view: EditorView): TextRange[] {
  const length = view.state.doc.length;
  const visible = view.visibleRanges.length > 0 ? view.visibleRanges : [view.viewport];
  return mergeRanges(
    visible.map(({ from, to }) => ({
      from: Math.max(0, from - VIEWPORT_BUFFER),
      to: Math.min(length, to + VIEWPORT_BUFFER),
    })),
  );
}

function sourceConstructAt(state: EditorState, position: number, side: -1 | 1): TextRange | null {
  let node: MarkdownSyntaxNode | null = syntaxTree(state).resolveInner(position, side);
  while (node) {
    if (SOURCE_CONSTRUCTS.has(node.name)) return { from: node.from, to: node.to };
    node = node.parent;
  }
  return null;
}

function activeSourceRanges(state: EditorState): TextRange[] {
  const ranges: TextRange[] = [];
  for (const selection of state.selection.ranges) {
    const startLine = state.doc.lineAt(selection.from);
    const endLine = state.doc.lineAt(selection.to);
    ranges.push({ from: startLine.from, to: Math.max(startLine.to, endLine.to) });

    for (const position of new Set([selection.anchor, selection.head])) {
      for (const side of [-1, 1] as const) {
        const construct = sourceConstructAt(state, position, side);
        if (construct) ranges.push(construct);
      }
    }
  }
  return mergeRanges(ranges);
}

function intersects(ranges: readonly TextRange[], from: number, to: number): boolean {
  return ranges.some((range) => range.from < to && range.to > from);
}

function clippedRanges(ranges: readonly TextRange[], from: number, to: number): TextRange[] {
  const clipped: TextRange[] = [];
  for (const range of ranges) {
    const clippedFrom = Math.max(from, range.from);
    const clippedTo = Math.min(to, range.to);
    if (clippedTo > clippedFrom) clipped.push({ from: clippedFrom, to: clippedTo });
  }
  return clipped;
}

function nearestParent(node: MarkdownSyntaxNode, name: string): MarkdownSyntaxNode | null {
  let parent = node.parent;
  while (parent) {
    if (parent.name === name) return parent;
    if (parent.name === "Document" || parent.name === "Paragraph") return null;
    parent = parent.parent;
  }
  return null;
}

function isInlineLink(node: MarkdownSyntaxNode): boolean {
  return node.name === "Link" && node.getChild("URL") !== null;
}

function semanticDecoration(node: MarkdownSyntaxNode): Decoration | null {
  // This is deliberately an allowlist. Tables, images, raw HTML, and reference links remain
  // untouched source until they have editor-native semantics matching the Reading renderer.
  const heading = HEADING_NODE.exec(node.name);
  if (heading) return headingMarks[Number(heading[1]) - 1] ?? null;
  if (node.name === "StrongEmphasis") return semanticMarks.strong;
  if (node.name === "Emphasis") return semanticMarks.emphasis;
  if (node.name === "Strikethrough") return semanticMarks.strike;
  if (node.name === "InlineCode") return semanticMarks.inlineCode;
  if (node.name === "FencedCode") return semanticMarks.fencedCode;
  if (node.name === "Blockquote") return semanticMarks.blockquote;
  if (notesLinkNodeRole(node.name) === "link") return semanticMarks.link;
  if (node.name === "Autolink" || isInlineLink(node)) return semanticMarks.link;
  return null;
}

function syntaxDecoration(node: MarkdownSyntaxNode, state: EditorState): Decoration | null {
  const parent = node.parent;
  if (!parent) return null;
  if (node.name === "HeaderMark" && HEADING_NODE.test(parent.name)) return hiddenSyntax;
  if (
    node.name === "EmphasisMark" &&
    (parent.name === "Emphasis" || parent.name === "StrongEmphasis")
  ) {
    return hiddenSyntax;
  }
  if (node.name === "StrikethroughMark" && parent.name === "Strikethrough") {
    return hiddenSyntax;
  }
  if (node.name === "QuoteMark" && nearestParent(node, "Blockquote")) return hiddenSyntax;
  if (node.name === "CodeMark" && (parent.name === "InlineCode" || parent.name === "FencedCode")) {
    return hiddenSyntax;
  }
  if (node.name === "CodeInfo" && parent.name === "FencedCode") return hiddenSyntax;
  if (node.name === "ListMark" && parent.name === "ListItem") return listMarker;
  if (node.name === "TaskMarker" && parent.name === "Task") {
    const marker = state.doc.sliceString(node.from, node.to);
    const checked = /\[[xX]\]/.test(marker);
    return Decoration.replace({
      widget: new TaskCheckboxWidget(
        checked,
        taskLabel(state, node, parent),
        node.from,
        state.readOnly,
      ),
    });
  }
  if (notesLinkNodeRole(node.name) === "mark") return hiddenSyntax;
  if (node.name === "LinkMark") {
    if (parent.name === "Autolink") return hiddenSyntax;
    if (isInlineLink(parent)) return hiddenSyntax;
  }
  if (node.name === "URL" && isInlineLink(parent)) return hiddenSyntax;
  return null;
}

/**
 * Builds visual-only decorations. The Markdown document and selection remain the accessible,
 * copyable source of truth; unsupported syntax is intentionally never decorated.
 *
 * Exported for bounded deterministic tests. Production callers pass buffered visible ranges.
 */
export function buildDocumentLivePreviewDecorations(
  state: EditorState,
  visibleRanges: readonly TextRange[],
  revealSelection = true,
): DecorationSet {
  const visible = mergeRanges(visibleRanges);
  if (visible.length === 0) return Decoration.none;
  const active = revealSelection ? activeSourceRanges(state) : [];
  const decorations: Range<Decoration>[] = [];
  const tree = syntaxTree(state);

  for (const visibleRange of visible) {
    tree.iterate({
      from: visibleRange.from,
      to: visibleRange.to,
      enter(reference) {
        const node = reference.node;
        if (intersects(active, node.from, node.to)) return;

        const semantic = semanticDecoration(node);
        if (semantic) {
          for (const range of clippedRanges([visibleRange], node.from, node.to)) {
            decorations.push(semantic.range(range.from, range.to));
          }
        }

        const syntax = syntaxDecoration(node, state);
        if (syntax && node.from >= visibleRange.from && node.to <= visibleRange.to) {
          decorations.push(syntax.range(node.from, node.to));
        }
      },
    });
  }

  return Decoration.set(decorations, true);
}

class DocumentLivePreviewPlugin {
  decorations: DecorationSet;

  constructor(view: EditorView) {
    this.decorations = buildDocumentLivePreviewDecorations(
      view.state,
      bufferedVisibleRanges(view),
      view.hasFocus,
    );
  }

  update(update: ViewUpdate) {
    // Language parsing advances asynchronously in otherwise empty state transactions. Without
    // observing the tree identity, a large document could stay partly raw until the next edit or
    // selection change even though CodeMirror had finished parsing it in the background.
    const treeChanged = syntaxTree(update.startState) !== syntaxTree(update.state);
    const readOnlyChanged = update.startState.readOnly !== update.state.readOnly;
    if (
      !update.docChanged &&
      !update.selectionSet &&
      !update.viewportChanged &&
      !update.focusChanged &&
      !treeChanged &&
      !readOnlyChanged
    ) {
      return;
    }
    this.decorations = buildDocumentLivePreviewDecorations(
      update.state,
      bufferedVisibleRanges(update.view),
      update.view.hasFocus,
    );
  }
}

const documentLivePreviewPlugin = ViewPlugin.fromClass(DocumentLivePreviewPlugin, {
  decorations: (plugin) => plugin.decorations,
});

const documentLivePreviewTheme = EditorView.baseTheme({
  ".cm-lp-hidden-syntax": {
    display: "none",
  },
  ".cm-lp-heading, .cm-lp-heading *": {
    fontWeight: "700",
    lineHeight: "1.35",
    textDecoration: "none",
  },
  ".cm-lp-heading-1": { fontSize: "1.8em" },
  ".cm-lp-heading-2": { fontSize: "1.5em" },
  ".cm-lp-heading-3": { fontSize: "1.3em" },
  ".cm-lp-heading-4": { fontSize: "1.15em" },
  ".cm-lp-heading-5": { fontSize: "1.05em" },
  ".cm-lp-heading-6": { fontSize: "1em" },
  ".cm-lp-strong": { fontWeight: "700" },
  ".cm-lp-emphasis": { fontStyle: "italic" },
  ".cm-lp-strike": { textDecoration: "line-through" },
  ".cm-lp-link": {
    color: "var(--primary)",
    textDecoration: "underline",
    textDecorationColor: "color-mix(in oklab, var(--primary) 45%, transparent)",
    textUnderlineOffset: "0.14em",
  },
  ".cm-lp-blockquote": {
    color: "var(--muted-foreground)",
    fontStyle: "italic",
  },
  ".cm-lp-inline-code": {
    borderRadius: "0.3rem",
    backgroundColor: "color-mix(in oklab, var(--muted) 72%, transparent)",
    padding: "0.08em 0.28em",
    fontFamily: "var(--font-mono, ui-monospace, monospace)",
    fontSize: "0.92em",
  },
  ".cm-lp-fenced-code": {
    backgroundColor: "color-mix(in oklab, var(--muted) 55%, transparent)",
    fontFamily: "var(--font-mono, ui-monospace, monospace)",
    fontSize: "0.92em",
  },
  ".cm-lp-list-marker": {
    color: "var(--primary)",
    fontWeight: "700",
  },
  ".cm-lp-task-checkbox": {
    display: "inline-flex",
    verticalAlign: "middle",
    margin: "0 0.4em 0.15em 0",
  },
  ".cm-lp-task-checkbox input": {
    cursor: "pointer",
    accentColor: "var(--primary)",
  },
  ".cm-lp-task-checkbox input:disabled": {
    cursor: "default",
  },
});

export function documentAuthoringExtensions(mode: DocumentAuthoringMode): Extension {
  return [
    EditorView.contentAttributes.of({ "data-document-authoring-mode": mode }),
    mode === "live_preview" ? [documentLivePreviewPlugin, documentLivePreviewTheme] : [],
  ];
}
