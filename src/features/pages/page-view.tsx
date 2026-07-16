import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Clock3, Loader2, MoreHorizontal, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Outliner } from "@/features/outliner/outliner";
import { AttachmentsCard } from "@/features/attachments/attachments-card";
import { renamePage, type Node } from "@/lib/api";
import { DebouncedAction } from "@/lib/debounced-action";

type Props = {
  node: Node;
  onSaved: (updated: Node) => void;
  onStatus: (s: string) => void;
  onClose: () => void;
  onDelete: (node: Node) => void | Promise<void>;
  initialBlockId?: number | null;
  autoFocusTitle?: boolean;
};

type SaveState = "idle" | "dirty" | "saving" | "saved" | "error";

const AUTOSAVE_MS = 400;

export function PageView({
  node,
  onSaved,
  onStatus,
  onClose,
  onDelete,
  initialBlockId = null,
  autoFocusTitle = false,
}: Props) {
  const [title, setTitle] = useState(node.title ?? "");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [focusBody, setFocusBody] = useState(false);

  const autosave = useRef(new DebouncedAction()).current;
  const titleInput = useRef<HTMLInputElement>(null);
  const nodeRef = useRef(node);
  const titleRef = useRef(title);
  const onSavedRef = useRef(onSaved);
  const onStatusRef = useRef(onStatus);

  titleRef.current = title;
  onSavedRef.current = onSaved;
  onStatusRef.current = onStatus;

  const flush = useCallback(async () => {
    autosave.cancel();
    const current = nodeRef.current;
    const nextTitle = titleRef.current.trim() || null;
    if (nextTitle === (current.title ?? null)) {
      setSaveState("idle");
      return;
    }
    setSaveState("saving");
    try {
      const updated = await renamePage(current.uuid, nextTitle);
      nodeRef.current = updated;
      onSavedRef.current(updated);
      setSaveState("saved");
    } catch (err) {
      setSaveState("error");
      onStatusRef.current(`save error: ${String(err)}`);
    }
  }, [autosave]);

  const scheduleSave = useCallback(() => {
    setSaveState("dirty");
    autosave.schedule(() => void flush(), AUTOSAVE_MS);
  }, [autosave, flush]);

  useEffect(() => {
    autosave.cancel();
    nodeRef.current = node;
    setTitle(node.title ?? "");
    setSaveState("idle");
    setFocusBody(false);
  }, [autosave, node]);

  useEffect(() => {
    if (!autoFocusTitle) return;
    const input = titleInput.current;
    input?.focus();
    input?.select();
  }, [autoFocusTitle, node.id]);

  useEffect(() => {
    return () => {
      if (autosave.cancel()) {
        void flush();
      }
    };
  }, [autosave, flush]);

  return (
    <article className="editor-page mx-auto flex min-h-full max-w-[52rem] flex-col px-8 pt-12 pb-24 sm:px-12 lg:px-16">
      <div className="mb-8 flex items-center gap-2 text-[11px] font-medium text-muted-foreground">
        <button type="button" onClick={onClose} className="transition hover:text-foreground">
          All notes
        </button>
        <span>/</span>
        <span className="truncate">{title || "Untitled"}</span>
        <span className="ml-auto flex items-center gap-1.5">
          <Clock3 className="size-3" />
          {new Date(node.updated_at * 1000).toLocaleDateString(undefined, {
            month: "short",
            day: "numeric",
          })}
        </span>
      </div>

      <div className="group flex items-start gap-3">
        <Input
          ref={titleInput}
          value={title}
          onBlur={() => void flush()}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              void flush();
              setFocusBody(true);
              event.currentTarget.blur();
            }
          }}
          onChange={(e) => {
            setTitle(e.currentTarget.value);
            scheduleSave();
          }}
          placeholder="Untitled note"
          className="h-[3.5rem] min-w-0 border-0 bg-transparent px-0 py-1 text-[2.6rem] leading-tight font-semibold tracking-[-0.045em] shadow-none placeholder:text-muted-foreground/35 focus-visible:ring-0"
        />
        <div className="mt-2 flex items-center gap-1">
          <SaveIndicator state={saveState} />
          <Button
            variant="ghost"
            size="icon-sm"
            className="rounded-lg text-muted-foreground opacity-0 transition group-hover:opacity-100 focus:opacity-100"
            aria-label="More note actions"
          >
            <MoreHorizontal className="size-4" />
          </Button>
        </div>
      </div>

      <div className="mt-4 mb-9 flex items-center gap-2">
        <span className="rounded-full bg-primary/10 px-2.5 py-1 text-[10px] font-semibold tracking-wide text-primary">
          NOTE
        </span>
        <Button
          variant="ghost"
          size="xs"
          className="ml-auto rounded-lg text-muted-foreground opacity-60 hover:text-destructive hover:opacity-100"
          aria-label="Delete page"
          onClick={() => void onDelete(node)}
        >
          <Trash2 className="size-4" />
        </Button>
      </div>

      <div className="editor-body">
        <Outliner key={node.id} page={node} initialEditingId={focusBody ? initialBlockId : null} />
      </div>
      <div className="mt-16">
        <AttachmentsCard parentId={node.id} parentUuid={node.uuid} onStatus={onStatus} />
      </div>
    </article>
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
