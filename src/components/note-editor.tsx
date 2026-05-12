import "@blocknote/core/fonts/inter.css";
import "@blocknote/shadcn/style.css";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/shadcn";
import type { PartialBlock } from "@blocknote/core";
import { forwardRef, useCallback, useEffect, useImperativeHandle } from "react";

export type NoteEditorHandle = {
  serialize: () => Promise<{ markdown: string; json: string }>;
  reset: () => void;
};

type Props = {
  /** Structured BlockNote document (preferred). */
  initialJson?: string | null;
  /** Plain text fallback when no structured form exists. */
  initialPlain?: string | null;
  placeholder?: string;
  /** Fires after every edit. Debounce on the caller side for autosave. */
  onChange?: () => void;
};

function paragraphsFromPlain(text: string): PartialBlock[] {
  const lines = text.split(/\n+/).filter((s) => s.trim().length > 0);
  if (lines.length === 0) return [{ type: "paragraph", content: [] }];
  return lines.map((line) => ({ type: "paragraph", content: line }));
}

export const NoteEditor = forwardRef<NoteEditorHandle, Props>(
  ({ initialJson, initialPlain, placeholder, onChange }, ref) => {
    let initialContent: PartialBlock[] | undefined;
    if (initialJson) {
      try {
        initialContent = JSON.parse(initialJson) as PartialBlock[];
      } catch {
        initialContent = undefined;
      }
    }
    if (!initialContent && initialPlain) {
      initialContent = paragraphsFromPlain(initialPlain);
    }

    const editor = useCreateBlockNote({ initialContent });

    const serialize = useCallback(async () => {
      const blocks = editor.document;
      const markdown = await editor.blocksToMarkdownLossy(blocks);
      return { markdown, json: JSON.stringify(blocks) };
    }, [editor]);

    const reset = useCallback(() => {
      editor.replaceBlocks(editor.document, [{ type: "paragraph", content: [] }]);
    }, [editor]);

    useImperativeHandle(ref, () => ({ serialize, reset }), [serialize, reset]);

    useEffect(() => {
      if (!onChange) return;
      return editor.onChange(() => onChange());
    }, [editor, onChange]);

    return (
      <div className="rounded-md border bg-card">
        <BlockNoteView editor={editor} theme="light" data-placeholder={placeholder} />
      </div>
    );
  },
);

NoteEditor.displayName = "NoteEditor";
