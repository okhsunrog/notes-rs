import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import {
  createBlock,
  deleteBlock,
  moveBlock,
  replaceBlockRefs,
  searchBlocksFts,
  searchPagesByTitle,
  updateNode,
  type Node,
} from "@/lib/api";
import { BlockChildren } from "./block-tree";
import { BlockEdit, type BlockEditHandle } from "./block-edit";
import { useOutliner } from "./outliner-store";
import { nextSibling, positionAfter, prevSibling } from "./keyboard";
import { parseRefs } from "./parse-refs";
import { renderMarkdown } from "./render-markdown";
import { detectTrigger, type Trigger } from "./autocomplete";
import { AutocompleteMenu, nodeToItem, type AutocompleteItem } from "./autocomplete-menu";

type Props = {
  block: Node;
  parent: Node;
  depth: number;
};

type SaveState = "idle" | "dirty" | "saving" | "error";

const AUTOSAVE_MS = 400;
const AC_DEBOUNCE_MS = 120;

export function BlockNode({ block, parent, depth }: Props) {
  const store = useOutliner();
  const editing = store.editingId === block.id;
  const [collapsed, setCollapsed] = useState(false);
  const [saveState, setSaveState] = useState<SaveState>("idle");

  const draftRef = useRef(block.content);
  const blockRef = useRef(block);
  blockRef.current = block;
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const editRef = useRef<BlockEditHandle>(null);

  // Autocomplete state — only relevant in edit mode.
  const [trigger, setTrigger] = useState<Trigger | null>(null);
  const [acItems, setAcItems] = useState<AutocompleteItem[]>([]);
  const [acIdx, setAcIdx] = useState(0);
  const [acLoading, setAcLoading] = useState(false);
  const acReqId = useRef(0);
  const acDebounce = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearTimer = () => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  };

  const closeAutocomplete = () => {
    setTrigger(null);
    setAcItems([]);
    setAcIdx(0);
    setAcLoading(false);
    if (acDebounce.current) {
      clearTimeout(acDebounce.current);
      acDebounce.current = null;
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
      const { wikilinks, blockRefs } = parseRefs(next);
      replaceBlockRefs({
        blockId: current.id,
        wikilinkTitles: wikilinks,
        blockUuids: blockRefs,
      }).catch((e) => console.error("ref replace failed", e));
    } catch (err) {
      console.error("block save failed", err);
      setSaveState("error");
    }
  }, [store]);

  // Re-fetch results whenever the trigger query changes.
  useEffect(() => {
    if (!trigger) return;
    if (acDebounce.current) clearTimeout(acDebounce.current);
    const reqId = ++acReqId.current;
    setAcLoading(true);
    acDebounce.current = setTimeout(async () => {
      try {
        if (trigger.kind === "[[") {
          const nodes = await searchPagesByTitle(trigger.query, 8);
          if (reqId !== acReqId.current) return;
          setAcItems(nodes.map((n) => nodeToItem(n, "[[")));
        } else {
          const ftsQuery = buildFtsPrefix(trigger.query);
          const nodes = ftsQuery ? await searchBlocksFts(ftsQuery, 8) : [];
          if (reqId !== acReqId.current) return;
          setAcItems(nodes.map((n) => nodeToItem(n, "((")));
        }
        if (reqId === acReqId.current) {
          setAcIdx(0);
          setAcLoading(false);
        }
      } catch (e) {
        if (reqId !== acReqId.current) return;
        console.error("autocomplete fetch failed", e);
        setAcItems([]);
        setAcLoading(false);
      }
    }, AC_DEBOUNCE_MS);
    return () => {
      if (acDebounce.current) clearTimeout(acDebounce.current);
    };
  }, [trigger]);

  const onDraftChange = (value: string, caret: number) => {
    draftRef.current = value;
    setSaveState("dirty");
    clearTimer();
    timer.current = setTimeout(() => {
      timer.current = null;
      void flush();
    }, AUTOSAVE_MS);
    const t = detectTrigger(value, caret);
    if (!t) {
      if (trigger) closeAutocomplete();
      return;
    }
    if (
      !trigger ||
      trigger.kind !== t.kind ||
      trigger.start !== t.start ||
      trigger.query !== t.query
    ) {
      setTrigger(t);
    }
  };

  const acceptAutocomplete = (idx: number) => {
    if (!trigger || !editRef.current) return;
    const item = acItems[idx];
    if (!item) return;
    const replacement = trigger.kind === "[[" ? `[[${item.label}]]` : `((${item.label}))`;
    editRef.current.replaceRange(trigger.start, trigger.end, replacement);
    closeAutocomplete();
  };

  const onBlur = () => {
    closeAutocomplete();
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
      if (!ok) return false;
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
    if (!prev) return;
    try {
      const moved = await moveBlock({
        id: block.id,
        newParentId: prev.id,
        newPosition: null,
      });
      store.moveLocal(moved, parent.id);
      store.setEditing(moved.id);
    } catch (e) {
      console.error("tab indent failed", e);
    }
  };

  const onShiftTab = async () => {
    if (parent.kind === "page") return;
    const grandparentId = parent.parent_id;
    if (grandparentId === null) return;
    await flush();
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
    // While the autocomplete menu is open, it captures navigation keys.
    if (trigger) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setAcIdx((i) => (acItems.length ? (i + 1) % acItems.length : 0));
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setAcIdx((i) => (acItems.length ? (i - 1 + acItems.length) % acItems.length : 0));
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        if (acItems.length > 0) {
          e.preventDefault();
          acceptAutocomplete(acIdx);
          return;
        }
        // Empty results: close and let the key do its normal thing for Enter.
        closeAutocomplete();
        if (e.key === "Tab") {
          e.preventDefault();
          return;
        }
      }
      if (e.key === "Escape") {
        e.preventDefault();
        closeAutocomplete();
        return;
      }
      // Any other key falls through (so typing continues to update the query).
    }

    if (e.key === "Enter" && !e.shiftKey && !e.ctrlKey && !e.metaKey) {
      e.preventDefault();
      void onEnter();
      return;
    }
    if (e.key === "Backspace" && draftRef.current.length === 0) {
      e.preventDefault();
      void onBackspaceEmpty();
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      if (e.shiftKey) void onShiftTab();
      else void onTab();
      return;
    }
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
          className="relative min-w-0 flex-1"
          onClick={() => {
            if (!editing) {
              draftRef.current = block.content;
              store.setEditing(block.id);
            }
          }}
        >
          {editing ? (
            <>
              <BlockEdit
                ref={editRef}
                initial={block.content}
                onChange={onDraftChange}
                onBlur={onBlur}
                onKeyDown={onKeyDown}
                autoFocus
              />
              {trigger && (
                <AutocompleteMenu
                  items={acItems}
                  selectedIdx={acIdx}
                  loading={acLoading}
                  query={trigger.query}
                  emptyLabel={
                    trigger.kind === "[["
                      ? `no pages match — press Enter to skip`
                      : `no blocks match`
                  }
                  onPick={acceptAutocomplete}
                />
              )}
            </>
          ) : (
            <div className="cursor-text whitespace-pre-wrap break-words text-sm leading-relaxed">
              {block.content ? (
                renderMarkdown(block.content)
              ) : (
                <span className="text-muted-foreground/40">empty</span>
              )}
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

/** Convert "auto comp" → "auto* comp*" for FTS5 prefix matching. Strips
 * characters that would break the FTS5 expression (operators are bare words,
 * so we keep alphanumerics + Cyrillic). */
function buildFtsPrefix(q: string): string {
  return q
    .split(/\s+/)
    .map((t) => t.replace(/[^\p{L}\p{N}]+/gu, ""))
    .filter((t) => t.length > 0)
    .map((t) => `${t}*`)
    .join(" ");
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
