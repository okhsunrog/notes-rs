import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft, Check, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NoteEditor, type NoteEditorHandle } from "@/components/note-editor";
import { updateNode, type Node } from "@/lib/api";

type Props = {
  node: Node;
  onSaved: (updated: Node) => void;
  onStatus: (s: string) => void;
  onClose: () => void;
};

type SaveState = "idle" | "dirty" | "saving" | "saved" | "error";

const AUTOSAVE_MS = 400;

export function PageView({ node, onSaved, onStatus, onClose }: Props) {
  const [title, setTitle] = useState(node.title ?? "");
  const [saveState, setSaveState] = useState<SaveState>("idle");

  // Refs so the flush function never depends on changing values — avoids
  // stale-closure races when the user types fast or switches nodes mid-save.
  const editorRef = useRef<NoteEditorHandle>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const nodeRef = useRef(node);
  const titleRef = useRef(title);
  const onSavedRef = useRef(onSaved);
  const onStatusRef = useRef(onStatus);

  // Keep refs in sync with latest props/state (no deps needed at flush time).
  titleRef.current = title;
  onSavedRef.current = onSaved;
  onStatusRef.current = onStatus;

  const flush = useCallback(async () => {
    if (!editorRef.current) return;
    const current = nodeRef.current;
    const { markdown, json } = await editorRef.current.serialize();
    const nextTitle = titleRef.current.trim() || null;
    setSaveState("saving");
    try {
      await updateNode({
        id: current.id,
        title: nextTitle,
        content: markdown.trim(),
        contentJson: json,
      });
      const updated: Node = {
        ...current,
        title: nextTitle,
        content: markdown.trim(),
        content_json: json,
        updated_at: Math.floor(Date.now() / 1000),
      };
      nodeRef.current = updated;
      onSavedRef.current(updated);
      setSaveState("saved");
    } catch (err) {
      setSaveState("error");
      onStatusRef.current(`save error: ${err}`);
    }
  }, []);

  const scheduleSave = useCallback(() => {
    setSaveState("dirty");
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      timer.current = null;
      void flush();
    }, AUTOSAVE_MS);
  }, [flush]);

  // When the active node changes: drop any pending save (the editor is about
  // to remount with new content via `key={node.id}`, so calling flush here
  // would serialize the NEW editor into the OLD row).
  useEffect(() => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    nodeRef.current = node;
    setTitle(node.title ?? "");
    setSaveState("idle");
  }, [node]);

  // Flush on unmount (editor is still mounted at this point).
  useEffect(() => {
    return () => {
      if (timer.current) {
        clearTimeout(timer.current);
        timer.current = null;
        void flush();
      }
    };
  }, [flush]);

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-4">
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="sm" onClick={onClose} title="Back">
          <ArrowLeft className="size-4" />
        </Button>
        <Input
          value={title}
          onChange={(e) => {
            setTitle(e.currentTarget.value);
            scheduleSave();
          }}
          placeholder="title"
          className="h-10 border-0 bg-transparent px-0 text-2xl font-semibold shadow-none focus-visible:ring-0"
        />
        <SaveIndicator state={saveState} />
      </div>

      <div className="text-xs text-muted-foreground">
        Node #{node.id} · {node.kind} · updated {new Date(node.updated_at * 1000).toLocaleString()}
      </div>

      <NoteEditor
        key={node.id}
        ref={editorRef}
        initialJson={node.content_json}
        initialPlain={node.content}
        placeholder="start writing…"
        onChange={scheduleSave}
      />
    </div>
  );
}

function SaveIndicator({ state }: { state: SaveState }) {
  if (state === "idle") return null;
  if (state === "dirty") return <span className="text-xs text-muted-foreground">unsaved…</span>;
  if (state === "saving")
    return (
      <span className="flex items-center gap-1 text-xs text-muted-foreground">
        <Loader2 className="size-3 animate-spin" />
        saving
      </span>
    );
  if (state === "saved")
    return (
      <span className="flex items-center gap-1 text-xs text-emerald-600">
        <Check className="size-3" />
        saved
      </span>
    );
  return <span className="text-xs text-destructive">save failed</span>;
}
