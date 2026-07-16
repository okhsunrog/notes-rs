import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft, Check, Loader2, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Outliner } from "@/features/outliner/outliner";
import { AttachmentsCard } from "@/features/attachments/attachments-card";
import { updateNode, type Node } from "@/lib/api";

type Props = {
  node: Node;
  onSaved: (updated: Node) => void;
  onStatus: (s: string) => void;
  onClose: () => void;
  onDelete: (node: Node) => void | Promise<void>;
};

type SaveState = "idle" | "dirty" | "saving" | "saved" | "error";

const AUTOSAVE_MS = 400;

export function PageView({ node, onSaved, onStatus, onClose, onDelete }: Props) {
  const [title, setTitle] = useState(node.title ?? "");
  const [saveState, setSaveState] = useState<SaveState>("idle");

  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const nodeRef = useRef(node);
  const titleRef = useRef(title);
  const onSavedRef = useRef(onSaved);
  const onStatusRef = useRef(onStatus);

  titleRef.current = title;
  onSavedRef.current = onSaved;
  onStatusRef.current = onStatus;

  const flush = useCallback(async () => {
    const current = nodeRef.current;
    const nextTitle = titleRef.current.trim() || null;
    if (nextTitle === (current.title ?? null)) {
      setSaveState("idle");
      return;
    }
    setSaveState("saving");
    try {
      await updateNode({
        id: current.id,
        title: nextTitle,
        content: current.content,
        contentJson: current.content_json,
      });
      const updated: Node = {
        ...current,
        title: nextTitle,
        updated_at: Math.floor(Date.now() / 1000),
      };
      nodeRef.current = updated;
      onSavedRef.current(updated);
      setSaveState("saved");
    } catch (err) {
      setSaveState("error");
      onStatusRef.current(`save error: ${String(err)}`);
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

  useEffect(() => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    nodeRef.current = node;
    setTitle(node.title ?? "");
    setSaveState("idle");
  }, [node]);

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
          onBlur={() => void flush()}
          onChange={(e) => {
            setTitle(e.currentTarget.value);
            scheduleSave();
          }}
          placeholder="title"
          className="h-10 border-0 bg-transparent px-0 text-2xl font-semibold shadow-none focus-visible:ring-0"
        />
        <SaveIndicator state={saveState} />
        <Button
          variant="ghost"
          size="sm"
          className="ml-auto text-muted-foreground hover:text-destructive"
          aria-label="Delete page"
          onClick={() => void onDelete(node)}
        >
          <Trash2 className="size-4" />
        </Button>
      </div>

      <div className="text-xs text-muted-foreground">
        Node #{node.id} · {node.kind} · updated {new Date(node.updated_at * 1000).toLocaleString()}
      </div>

      <Outliner key={node.id} page={node} />
      <AttachmentsCard parentId={node.id} onStatus={onStatus} />
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
