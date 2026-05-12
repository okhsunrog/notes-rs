import { useCallback, useRef, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { createBlock, deleteBlock, moveBlock, updateNode, type Node } from "@/lib/api";
import { BlockChildren } from "./block-tree";
import { BlockEdit } from "./block-edit";
import { useOutliner } from "./outliner-store";
import { nextSibling, positionAfter, prevSibling } from "./keyboard";

type Props = {
  block: Node;
  parent: Node;
  depth: number;
};

type SaveState = "idle" | "dirty" | "saving" | "error";

const AUTOSAVE_MS = 400;

export function BlockNode({ block, parent, depth }: Props) {
  const store = useOutliner();
  const editing = store.editingId === block.id;
  const [collapsed, setCollapsed] = useState(false);
  const [saveState, setSaveState] = useState<SaveState>("idle");

  const draftRef = useRef(block.content);
  const blockRef = useRef(block);
  blockRef.current = block;
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearTimer = () => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  };

  const flush = useCallback(async () => {
    clearTimer();
    const current = blockRef.current;
    const next = draftRef.current;
    if (next === current.content) {
      setSaveState("idle");
      return;
    }
    setSaveState("saving");
    try {
      await updateNode({
        id: current.id,
        title: current.title,
        content: next,
        contentJson: null,
      });
      const updated: Node = {
        ...current,
        content: next,
        content_json: null,
        updated_at: Math.floor(Date.now() / 1000),
      };
      store.replaceBlock(updated);
      setSaveState("idle");
    } catch (err) {
      console.error("block save failed", err);
      setSaveState("error");
    }
  }, [store]);

  const onDraftChange = (value: string) => {
    draftRef.current = value;
    setSaveState("dirty");
    clearTimer();
    timer.current = setTimeout(() => {
      timer.current = null;
      void flush();
    }, AUTOSAVE_MS);
  };

  const onBlur = () => {
    // Blur saves but does NOT exit edit mode unconditionally — if focus moved
    // to another block via keyboard, the store's editingId already changed and
    // that block will mount in edit mode. Just flush here.
    void flush();
  };

  const onEnter = async () => {
    await flush();
    const siblings = store.getChildren(parent.id) ?? [];
    const pos = positionAfter(siblings, block.id);
    try {
      const created = await createBlock({
        parentId: parent.id,
        position: pos,
        content: "",
        contentJson: null,
      });
      store.insertAfter(parent.id, block.id, created);
      store.setEditing(created.id);
    } catch (e) {
      console.error("enter (new sibling) failed", e);
    }
  };

  const onBackspaceEmpty = async () => {
    if (draftRef.current.length > 0) return false;
    const siblings = store.getChildren(parent.id) ?? [];
    const prev = prevSibling(siblings, block.id);
    try {
      const ok = await deleteBlock(block.id);
      if (!ok) return false; // has children — refuse silently
      store.removeBlock(parent.id, block.id);
      if (prev) store.setEditing(prev.id);
      else store.setEditing(null);
      return true;
    } catch (e) {
      console.error("backspace delete failed", e);
      return false;
    }
  };

  const onTab = async () => {
    await flush();
    const siblings = store.getChildren(parent.id) ?? [];
    const prev = prevSibling(siblings, block.id);
    if (!prev) return; // no-op: nothing to indent under
    try {
      const moved = await moveBlock({
        id: block.id,
        newParentId: prev.id,
        newPosition: null, // append to new parent's end
      });
      store.moveLocal(moved, parent.id);
      // Stay in edit mode on the moved block; BlockNode will remount under the
      // new parent thanks to the store update.
      store.setEditing(moved.id);
    } catch (e) {
      console.error("tab indent failed", e);
    }
  };

  const onShiftTab = async () => {
    if (parent.kind === "page") return; // already at top level
    const grandparentId = parent.parent_id;
    if (grandparentId === null) return;
    await flush();
    // Place just after the old parent in the grandparent's children.
    const newPos = (parent.position ?? 0) + 0.5;
    try {
      const moved = await moveBlock({
        id: block.id,
        newParentId: grandparentId,
        newPosition: newPos,
      });
      store.moveLocal(moved, parent.id);
      store.setEditing(moved.id);
    } catch (e) {
      console.error("shift-tab outdent failed", e);
    }
  };

  const onUpDown = (dir: "up" | "down") => {
    const siblings = store.getChildren(parent.id) ?? [];
    const target = dir === "up" ? prevSibling(siblings, block.id) : nextSibling(siblings, block.id);
    if (target) store.setEditing(target.id);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Enter (no shift) → new sibling below
    if (e.key === "Enter" && !e.shiftKey && !e.ctrlKey && !e.metaKey) {
      e.preventDefault();
      void onEnter();
      return;
    }
    // Backspace on empty → delete
    if (e.key === "Backspace" && draftRef.current.length === 0) {
      e.preventDefault();
      void onBackspaceEmpty();
      return;
    }
    // Tab / Shift+Tab → indent / outdent
    if (e.key === "Tab") {
      e.preventDefault();
      if (e.shiftKey) void onShiftTab();
      else void onTab();
      return;
    }
    // Arrow up/down at caret start/end → move focus to prev/next sibling
    if (e.key === "ArrowUp" && e.currentTarget.selectionStart === 0) {
      e.preventDefault();
      onUpDown("up");
      return;
    }
    if (e.key === "ArrowDown" && e.currentTarget.selectionEnd === e.currentTarget.value.length) {
      e.preventDefault();
      onUpDown("down");
      return;
    }
    // Escape → exit edit mode without saving content beyond what's flushed.
    if (e.key === "Escape") {
      e.preventDefault();
      void flush();
      store.setEditing(null);
    }
  };

  return (
    <li className="flex flex-col">
      <div className="group flex items-start gap-1 py-0.5">
        <button
          type="button"
          className="mt-1.5 flex size-4 shrink-0 items-center justify-center text-muted-foreground/50 hover:text-foreground"
          onClick={() => setCollapsed((c) => !c)}
          title={collapsed ? "expand" : "collapse"}
        >
          {collapsed ? (
            <ChevronRight className="size-3" />
          ) : (
            <ChevronDown className="size-3 opacity-0 group-hover:opacity-100" />
          )}
        </button>
        <BlockBullet state={saveState} />
        <div
          className="min-w-0 flex-1"
          onClick={() => {
            if (!editing) {
              draftRef.current = block.content;
              store.setEditing(block.id);
            }
          }}
        >
          {editing ? (
            <BlockEdit
              initial={block.content}
              onChange={onDraftChange}
              onBlur={onBlur}
              onKeyDown={onKeyDown}
              autoFocus
            />
          ) : (
            <div className="cursor-text whitespace-pre-wrap break-words text-sm leading-relaxed">
              {block.content || <span className="text-muted-foreground/40">empty</span>}
            </div>
          )}
        </div>
      </div>
      {!collapsed && (
        <div className="ml-5 border-l border-border/40 pl-2">
          <BlockChildren parent={block} depth={depth + 1} />
        </div>
      )}
    </li>
  );
}

function BlockBullet({ state }: { state: SaveState }) {
  const cls =
    state === "saving"
      ? "bg-amber-500/80 animate-pulse"
      : state === "dirty"
        ? "bg-amber-500/60"
        : state === "error"
          ? "bg-destructive"
          : "bg-muted-foreground/70";
  return <span className={`mt-2 size-1.5 shrink-0 rounded-full ${cls}`} />;
}
