// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { undo } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeAll, describe, expect, it, vi } from "vite-plus/test";
import { ContinuousDocumentEditor } from "./continuous-document-editor";
import type { DocumentAuthoringMode } from "./continuous-document-editor";
import { buildDocumentLivePreviewDecorations } from "./document-live-preview";

const cleanup: Array<() => void> = [];

beforeAll(() => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  for (const dispose of cleanup.splice(0)) dispose();
});

function editorView(container: HTMLElement): EditorView {
  const editor = container.querySelector<HTMLElement>(".cm-editor");
  const view = editor ? EditorView.findFromDOM(editor) : null;
  if (!view) throw new Error("CodeMirror view was not mounted");
  return view;
}

function editorProps(value: string, mode: DocumentAuthoringMode) {
  return {
    value,
    mode,
    readOnly: false,
    focusRequest: 0,
    onChange: vi.fn(),
    onCompositionEnd: vi.fn(),
    onBlur: vi.fn(),
  } as const;
}

async function mountEditor(value: string, mode: DocumentAuthoringMode = "live_preview") {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const props = editorProps(value, mode);
  cleanup.push(() => {
    act(() => root.unmount());
    container.remove();
  });
  await act(async () => root.render(<ContinuousDocumentEditor {...props} />));
  return { container, root, props, view: editorView(container) };
}

describe("Document Live Preview", () => {
  it("leaves source mode undecorated and source-visible", async () => {
    const source = "plain\n# Heading with **strong** and [link](https://example.com)";
    const { container, view } = await mountEditor(source, "source");

    const content = container.querySelector<HTMLElement>(".cm-content");
    expect(content?.dataset.documentAuthoringMode).toBe("source");
    expect(content?.textContent).toContain("# Heading with **strong**");
    expect(view.state.doc.toString()).toBe(source);
    expect(container.querySelector("[data-live-preview-hidden]")).toBeNull();
    expect(container.querySelector(".cm-lp-heading")).toBeNull();
  });

  it("hides known punctuation and applies semantic styles outside the active source", async () => {
    const blockUuid = "019c8d1a-4ab1-7f31-8f00-f594337c3ca5";
    const source = [
      "plain",
      "# Heading **strong** *emphasis* ~~strike~~ [link](https://example.com)",
      `Internal [[Project Aurora]] and ((${blockUuid}))`,
      "> quote with `code`",
      "- [ ] task",
      "```ts",
      "const answer = 42",
      "```",
    ].join("\n");
    const { container, view } = await mountEditor(source);

    const hiddenSource = [...container.querySelectorAll<HTMLElement>(".cm-lp-hidden-syntax")].map(
      (element) => element.textContent,
    );
    expect(hiddenSource).toContain("#");
    expect(hiddenSource).toContain("**");
    expect(hiddenSource).toContain("~~");
    expect(hiddenSource).toContain("https://example.com");
    expect(hiddenSource).toContain("[[");
    expect(hiddenSource).toContain("((");
    expect(hiddenSource).toContain(">");
    expect(hiddenSource).toContain("```");
    const hiddenHeadingMarker = [
      ...container.querySelectorAll<HTMLElement>(".cm-lp-hidden-syntax"),
    ].find((element) => element.textContent === "#");
    expect(hiddenHeadingMarker).toBeDefined();
    expect(getComputedStyle(hiddenHeadingMarker!).display).toBe("none");
    expect(container.querySelector(".cm-lp-heading-1")).not.toBeNull();
    expect(container.querySelector(".cm-lp-strong")).not.toBeNull();
    expect(container.querySelector(".cm-lp-emphasis")).not.toBeNull();
    expect(container.querySelector(".cm-lp-strike")).not.toBeNull();
    expect(container.querySelector(".cm-lp-link")).not.toBeNull();
    expect(container.querySelectorAll(".cm-lp-link").length).toBeGreaterThanOrEqual(3);
    expect(container.querySelector(".cm-lp-blockquote")).not.toBeNull();
    expect(container.querySelector(".cm-lp-inline-code")).not.toBeNull();
    expect(container.querySelector(".cm-lp-fenced-code")).not.toBeNull();
    expect(container.querySelector('[data-live-preview-marker="list"]')).not.toBeNull();
    expect(container.querySelector('[data-live-preview-marker="task"]')).not.toBeNull();
    expect(view.state.doc.toString()).toBe(source);
    expect(view.state.sliceDoc(0, view.state.doc.length)).toBe(source);
  });

  it("reveals the complete notes-link source under the caret", async () => {
    const source = "plain\nOpen [[Project Aurora]] beside text";
    const { container, view } = await mountEditor(source);

    expect(container.querySelectorAll("[data-live-preview-hidden]")).toHaveLength(2);
    act(() => {
      view.focus();
      view.dispatch({ selection: { anchor: source.indexOf("Aurora") } });
    });

    expect(container.querySelector("[data-live-preview-hidden]")).toBeNull();
    expect(container.querySelector(".cm-content")?.textContent).toContain("[[Project Aurora]]");
  });

  it("reveals complete source for the caret line and selected lines", async () => {
    const source = "plain\n**caret target**\n[selection](https://example.com)";
    const { container, view } = await mountEditor(source);
    const caret = source.indexOf("target");

    act(() => {
      view.focus();
      view.dispatch({ selection: { anchor: caret } });
    });
    const hiddenAtCaret = [...container.querySelectorAll<HTMLElement>(".cm-lp-hidden-syntax")].map(
      (element) => element.textContent,
    );
    expect(hiddenAtCaret).not.toContain("**");
    expect(hiddenAtCaret).toContain("https://example.com");
    expect(container.querySelector(".cm-lp-strong")).toBeNull();

    const selectionStart = source.indexOf("caret");
    act(() =>
      view.dispatch({
        selection: { anchor: selectionStart, head: source.length },
      }),
    );
    expect(container.querySelector(".cm-lp-hidden-syntax")).toBeNull();
    expect(container.querySelector(".cm-lp-link")).toBeNull();
  });

  it("renders every line while blurred and reveals source only in the focused editor", async () => {
    const source = "plain\n# Focused heading";
    const { container, view } = await mountEditor(source);
    const heading = source.indexOf("Focused");

    expect(container.querySelector(".cm-lp-hidden-syntax")?.textContent).toBe("#");
    expect(container.querySelector(".cm-lp-heading-1")).not.toBeNull();

    act(() => {
      view.focus();
      view.dispatch({ selection: { anchor: heading } });
    });
    expect(container.querySelector(".cm-lp-hidden-syntax")).toBeNull();
    expect(container.querySelector(".cm-lp-heading-1")).toBeNull();

    const outside = document.createElement("button");
    document.body.append(outside);
    cleanup.push(() => outside.remove());
    act(() => {
      outside.focus();
      // jsdom does not schedule CodeMirror's normal focus-change view update.
      view.dispatch({ selection: view.state.selection });
    });
    expect(container.querySelector(".cm-lp-hidden-syntax")?.textContent).toBe("#");
    expect(container.querySelector(".cm-lp-heading-1")).not.toBeNull();
  });

  it("switches modes in one EditorView without losing document, selection, or history", async () => {
    const initial = "plain\n**preview**";
    const { container, root, props, view } = await mountEditor(initial);
    const editor = container.querySelector(".cm-editor");
    const insertion = " retained";
    act(() => {
      view.dispatch({
        changes: { from: view.state.doc.length, insert: insertion },
        selection: { anchor: 8, head: 12 },
      });
    });
    const selection = view.state.selection.main;

    await act(async () => root.render(<ContinuousDocumentEditor {...props} mode="source" />));
    const sameView = editorView(container);
    expect(container.querySelector(".cm-editor")).toBe(editor);
    expect(sameView).toBe(view);
    expect(view.state.doc.toString()).toBe(initial + insertion);
    expect(view.state.selection.main.anchor).toBe(selection.anchor);
    expect(view.state.selection.main.head).toBe(selection.head);
    expect(
      container.querySelector(".cm-content")?.getAttribute("data-document-authoring-mode"),
    ).toBe("source");

    act(() => {
      expect(undo(view)).toBe(true);
    });
    expect(view.state.doc.toString()).toBe(initial);
  });

  it("keeps unsupported syntax recoverable and visible", async () => {
    const source = [
      "plain",
      "| a | b |",
      "| - | - |",
      "| c | d |",
      "<mark>raw html</mark>",
      "![alt](image.png)",
      "[reference][id]",
      "",
      "[id]: /target",
    ].join("\n");
    const { container } = await mountEditor(source);
    const renderedSource = container.querySelector(".cm-content")?.textContent;

    expect(renderedSource).toContain("| - | - |");
    expect(renderedSource).toContain("<mark>raw html</mark>");
    expect(renderedSource).toContain("![alt](image.png)");
    expect(renderedSource).toContain("[reference][id]");
    expect(renderedSource).toContain("[id]: /target");
    expect(container.querySelector(".cm-lp-hidden-syntax")).toBeNull();
  });

  it("bounds generated decorations to the supplied visible ranges", () => {
    const source = "plain\n**outside**\n**inside**\n**outside too**";
    const state = EditorState.create({
      doc: source,
      extensions: [markdown({ base: markdownLanguage })],
    });
    const visible = state.doc.line(3);
    const decorations = buildDocumentLivePreviewDecorations(state, [
      { from: visible.from, to: visible.to },
    ]);
    const positions: Array<{ from: number; to: number }> = [];
    decorations.between(0, state.doc.length, (from, to) => {
      positions.push({ from, to });
    });

    expect(positions.length).toBeGreaterThan(0);
    expect(positions.every(({ from, to }) => from >= visible.from && to <= visible.to)).toBe(true);
  });

  it("keeps the active line source-visible during composition", async () => {
    const source = "plain\n**composing**";
    const { container, view } = await mountEditor(source);
    const content = container.querySelector<HTMLElement>(".cm-content");
    const caret = source.indexOf("composing");
    act(() => {
      view.focus();
      view.dispatch({ selection: { anchor: caret } });
    });

    act(() => {
      content?.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    });
    expect(container.querySelector(".cm-lp-hidden-syntax")).toBeNull();

    act(() => {
      view.dispatch({ changes: { from: caret, insert: "я" } });
    });
    expect(container.querySelector(".cm-lp-hidden-syntax")).toBeNull();

    act(() => {
      content?.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
    });
  });

  it("applies a deferred mode switch only after composition has ended", async () => {
    const source = "plain\n**composing**";
    const { container, root, props, view } = await mountEditor(source);
    const content = container.querySelector<HTMLElement>(".cm-content");
    act(() => {
      view.focus();
      content?.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    });
    expect(view.compositionStarted).toBe(true);

    await act(async () => root.render(<ContinuousDocumentEditor {...props} mode="source" />));
    expect(content?.dataset.documentAuthoringMode).toBe("live_preview");

    await act(async () => {
      content?.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 24));
    });
    expect(view.compositionStarted).toBe(false);
    expect(content?.dataset.documentAuthoringMode).toBe("source");
    expect(view.state.doc.toString()).toBe(source);
  });

  it("stops retrying when an Android composition remains active", async () => {
    const source = "plain\n**long composition**";
    const { container, root, props, view } = await mountEditor(source);
    const content = container.querySelector<HTMLElement>(".cm-content");
    const compositionStarted = vi.spyOn(view, "compositionStarted", "get").mockReturnValue(true);
    vi.useFakeTimers();
    try {
      await act(async () => root.render(<ContinuousDocumentEditor {...props} mode="source" />));
      act(() => {
        content?.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
        vi.runAllTimers();
      });

      expect(content?.dataset.documentAuthoringMode).toBe("live_preview");
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      compositionStarted.mockRestore();
      vi.useRealTimers();
    }
  });

  it("renders an accessible checkbox widget for a task outside the caret", async () => {
    const source = "plain\n- [ ] Buy milk\n- [x] Done thing";
    const { container } = await mountEditor(source);

    const checkboxes = [
      ...container.querySelectorAll<HTMLInputElement>(
        '.cm-lp-task-checkbox input[type="checkbox"]',
      ),
    ];
    expect(checkboxes).toHaveLength(2);
    expect(checkboxes[0].checked).toBe(false);
    expect(checkboxes[0].getAttribute("aria-label")).toBe('Mark "Buy milk" as done');
    expect(checkboxes[1].checked).toBe(true);
    expect(checkboxes[1].getAttribute("aria-label")).toBe('Mark "Done thing" as not done');
    expect(container.querySelector(".cm-content")?.textContent).not.toContain("[ ]");
  });

  it("reveals the raw task marker as editable text under the caret", async () => {
    const source = "plain\n- [ ] Buy milk";
    const { container, view } = await mountEditor(source);
    act(() => {
      view.focus();
      view.dispatch({ selection: { anchor: source.indexOf("Buy") } });
    });

    expect(container.querySelector(".cm-lp-task-checkbox")).toBeNull();
    expect(container.querySelector(".cm-content")?.textContent).toContain("[ ] Buy milk");
  });

  it("toggles a task's checkbox by editing the marker character", async () => {
    const source = "plain\n- [ ] Buy milk";
    const { container, view } = await mountEditor(source);
    const checkbox = container.querySelector<HTMLInputElement>(
      '.cm-lp-task-checkbox input[type="checkbox"]',
    );
    expect(checkbox).not.toBeNull();

    act(() => {
      checkbox!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    });
    expect(view.state.doc.toString()).toBe("plain\n- [x] Buy milk");

    const toggledBack = container.querySelector<HTMLInputElement>(
      '.cm-lp-task-checkbox input[type="checkbox"]',
    );
    act(() => {
      toggledBack!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    });
    expect(view.state.doc.toString()).toBe("plain\n- [ ] Buy milk");
  });

  it("disables the checkbox and refuses to toggle a read-only document", async () => {
    const source = "plain\n- [ ] Buy milk";
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    cleanup.push(() => {
      act(() => root.unmount());
      container.remove();
    });
    await act(async () =>
      root.render(
        <ContinuousDocumentEditor
          value={source}
          readOnly
          focusRequest={0}
          onChange={() => undefined}
          onCompositionEnd={() => undefined}
          onBlur={() => undefined}
        />,
      ),
    );
    const checkbox = container.querySelector<HTMLInputElement>(
      '.cm-lp-task-checkbox input[type="checkbox"]',
    );
    expect(checkbox?.disabled).toBe(true);

    const view = editorView(container);
    act(() => {
      checkbox!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    });
    expect(view.state.doc.toString()).toBe(source);
  });
});
