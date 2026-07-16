import { useCallback, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ChevronDown, ChevronRight, Info } from "lucide-react";
import { toast } from "sonner";
import {
  createBlock,
  deleteBlock,
  indentBlock,
  moveBlockDown,
  moveBlockUp,
  outdentBlock,
  searchBlocksFts,
  searchPagesByTitle,
  splitBlock,
  setBlockContent,
  type BlockContent,
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
import { queryKeys } from "@/lib/query";
import { reconcileRemoteDraft } from "./editor-sync";

type Props = {
  block: Node;
  parent: Node;
  depth: number;
};

type SaveState = "idle" | "dirty" | "saving" | "error";

const AUTOSAVE_MS = 400;
const AC_DEBOUNCE_MS = 120;
const LONG_BLOCK_CHARS = 600;

function blockContent(content: string): BlockContent {
  const { wikilinks, blockRefs } = parseRefs(content);
  return { content, wikilinkTitles: wikilinks, blockUuids: blockRefs };
}

export function BlockNode({ block, parent, depth }: Props) {
  const store = useOutliner();
  const queryClient = useQueryClient();
  const editing = store.editingId === block.id;
  const collapseKey = `outliner.collapsed.${block.uuid}`;
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem(collapseKey) === "1");
  const [saveState, setSaveState] = useState<SaveState>("idle");

  const draftRef = useRef(block.content);
  const blockRef = useRef(block);
  const remoteConflictRef = useRef<Node | null>(null);
  const [remoteConflict, setRemoteConflict] = useState<Node | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const editRef = useRef<BlockEditHandle>(null);
  const [draftLen, setDraftLen] = useState(block.content.length);
  const longBlock = (editing ? draftLen : block.content.length) >= LONG_BLOCK_CHARS;

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

  useEffect(() => {
    const previous = blockRef.current;
    if (block.uuid !== previous.uuid) {
      blockRef.current = block;
      draftRef.current = block.content;
      setDraftLen(block.content.length);
      remoteConflictRef.current = null;
      setRemoteConflict(null);
      return;
    }
    const decision = reconcileRemoteDraft(previous.content, draftRef.current, block.content);
    if (decision === "unchanged") {
      blockRef.current = block;
      return;
    }

    if (decision === "conflict") {
      clearTimer();
      remoteConflictRef.current = block;
      setRemoteConflict(block);
      return;
    }

    blockRef.current = block;
    draftRef.current = block.content;
    setDraftLen(block.content.length);
    if (editing) store.setEditing(null);
  }, [block, editing, store]);

  const siblings = () => queryClient.getQueryData<Node[]>(queryKeys.children(parent.uuid)) ?? [];

  const invalidateChildren = (parentUuid?: string) =>
    queryClient.invalidateQueries({
      queryKey: parentUuid ? queryKeys.children(parentUuid) : queryKeys.childrenRoot,
    });

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
    if (remoteConflictRef.current) {
      setSaveState("error");
      return;
    }
    const current = blockRef.current;
    const next = draftRef.current;
    if (next === current.content) {
      setSaveState("idle");
      return;
    }
    setSaveState("saving");
    try {
      const [updated] = await setBlockContent(current.uuid, blockContent(next));
      blockRef.current = updated;
      queryClient.setQueryData<Node[]>(queryKeys.children(parent.uuid), (rows = []) =>
        rows.map((row) => (row.uuid === updated.uuid ? updated : row)),
      );
      queryClient.setQueryData(queryKeys.node(updated.uuid), updated);
      setSaveState("idle");
    } catch (err) {
      console.error("block save failed", err);
      setSaveState("error");
    }
  }, [parent.uuid, queryClient]);

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
    setDraftLen(value.length);
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

  const useRemoteVersion = () => {
    const remote = remoteConflictRef.current;
    if (!remote) return;
    clearTimer();
    blockRef.current = remote;
    draftRef.current = remote.content;
    setDraftLen(remote.content.length);
    remoteConflictRef.current = null;
    setRemoteConflict(null);
    setSaveState("idle");
    store.setEditing(null);
  };

  const keepLocalVersion = () => {
    const remote = remoteConflictRef.current;
    if (!remote) return;
    blockRef.current = remote;
    remoteConflictRef.current = null;
    setRemoteConflict(null);
    void flush();
  };

  const onPaste = async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const text = e.clipboardData.getData("text/plain");
    if (!text) return;
    const paragraphs = text
      .split(/\n[ \t]*(?:\n[ \t]*)+/)
      .map((p) => p.trim())
      .filter((p) => p.length > 0);
    if (paragraphs.length < 2) return; // single paragraph → default paste
    e.preventDefault();
    closeAutocomplete();

    // Splice paragraph 0 into the current block at the caret.
    const el = e.currentTarget;
    const selStart = el.selectionStart;
    const selEnd = el.selectionEnd;
    editRef.current?.replaceRange(selStart, selEnd, paragraphs[0]);

    clearTimer();
    try {
      const parts = [draftRef.current, ...paragraphs.slice(1)].map(blockContent);
      const changed = await splitBlock(block.id, parts);
      await invalidateChildren(parent.uuid);
      store.setEditing(changed[changed.length - 1]?.id ?? block.id);
    } catch (error) {
      console.error("paste split failed", error);
      setSaveState("error");
      return;
    }

    if (!localStorage.getItem("outliner.paste-split.notified")) {
      toast.info("Each paragraph became a separate block.", { duration: 4000 });
      localStorage.setItem("outliner.paste-split.notified", "1");
    }
  };

  /** Split this block's full content on blank-line paragraph breaks. Used by
   * the long-block info-icon nudge. If the content has no blank-line breaks,
   * we surface guidance instead of silently doing nothing. */
  const onSplitCurrent = async () => {
    const source = editing ? draftRef.current : block.content;
    const paragraphs = source
      .split(/\n[ \t]*(?:\n[ \t]*)+/)
      .map((p) => p.trim())
      .filter((p) => p.length > 0);
    if (paragraphs.length < 2) {
      toast.info("No paragraph breaks found — add blank lines between paragraphs first.");
      return;
    }
    clearTimer();
    try {
      const changed = await splitBlock(block.id, paragraphs.map(blockContent));
      draftRef.current = paragraphs[0];
      setDraftLen(paragraphs[0].length);
      await invalidateChildren(parent.uuid);
      store.setEditing(changed[changed.length - 1]?.id ?? block.id);
    } catch (err) {
      console.error("split failed", err);
      setSaveState("error");
      return;
    }
  };

  const onLongBlockNudge = () => {
    toast("Split this block into paragraphs?", {
      duration: 8000,
      action: {
        label: "Split",
        onClick: () => void onSplitCurrent(),
      },
    });
  };

  const onEnter = async () => {
    await flush();
    const pos = positionAfter(siblings(), block.id);
    try {
      const created = await createBlock({
        parentId: parent.id,
        position: pos,
        content: "",
        contentJson: null,
      });
      await invalidateChildren(parent.uuid);
      store.setEditing(created.id);
    } catch (e) {
      console.error("enter (new sibling) failed", e);
    }
  };

  const onBackspaceEmpty = async () => {
    if (draftRef.current.length > 0) return false;
    const prev = prevSibling(siblings(), block.id);
    try {
      const ok = await deleteBlock(block.id);
      if (!ok) return false;
      queryClient.setQueryData<Node[]>(queryKeys.children(parent.uuid), (rows = []) =>
        rows.filter((row) => row.uuid !== block.uuid),
      );
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
    try {
      const moved = await indentBlock(block.uuid);
      await invalidateChildren();
      store.setEditing(moved.id);
    } catch (e) {
      console.error("tab indent failed", e);
    }
  };

  const onShiftTab = async () => {
    if (parent.kind === "page") return;
    await flush();
    try {
      const moved = await outdentBlock(block.uuid);
      await invalidateChildren();
      store.setEditing(moved.id);
    } catch (e) {
      console.error("shift-tab outdent failed", e);
    }
  };

  const onUpDown = (dir: "up" | "down") => {
    const rows = siblings();
    const target = dir === "up" ? prevSibling(rows, block.id) : nextSibling(rows, block.id);
    if (target) store.setEditing(target.id);
  };

  const toggleCollapsed = () => {
    setCollapsed((value) => {
      const next = !value;
      localStorage.setItem(collapseKey, next ? "1" : "0");
      return next;
    });
  };

  const onReorder = async (direction: "up" | "down") => {
    await flush();
    try {
      await (direction === "up" ? moveBlockUp(block.uuid) : moveBlockDown(block.uuid));
      await invalidateChildren(parent.uuid);
      store.setEditing(block.id);
    } catch (error) {
      console.error("block reorder failed", error);
    }
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

    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      toggleCollapsed();
      return;
    }
    if ((e.ctrlKey || e.metaKey) && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
      e.preventDefault();
      void onReorder(e.key === "ArrowUp" ? "up" : "down");
      return;
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
      <div className="group -mx-2 flex items-start gap-1 rounded-xl px-2 py-1 transition-colors hover:bg-primary/[0.035] focus-within:bg-primary/[0.04]">
        <button
          type="button"
          className="mt-1.5 flex size-4 shrink-0 items-center justify-center rounded text-muted-foreground/35 transition hover:bg-primary/10 hover:text-primary"
          onClick={toggleCollapsed}
          title={collapsed ? "expand" : "collapse"}
        >
          {collapsed ? (
            <ChevronRight className="size-3" />
          ) : (
            <ChevronDown className="size-3 opacity-0 group-hover:opacity-100" />
          )}
        </button>
        <BlockBullet state={saveState} />
        {longBlock && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onLongBlockNudge();
            }}
            title="This block is getting long — click to split into paragraphs"
            className="mt-1.5 flex size-4 shrink-0 items-center justify-center text-amber-500 hover:text-amber-600"
          >
            <Info className="size-3" />
          </button>
        )}
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
                onPaste={onPaste}
                autoFocus
              />
              {remoteConflict && (
                <div
                  role="alert"
                  className="mt-2 flex flex-wrap items-center gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs"
                >
                  <span className="mr-auto text-foreground">
                    This block changed on another replica. Choose which version to keep.
                  </span>
                  <button
                    type="button"
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={useRemoteVersion}
                    className="rounded-md border bg-background px-2 py-1 hover:bg-accent"
                  >
                    Use remote
                  </button>
                  <button
                    type="button"
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={keepLocalVersion}
                    className="rounded-md bg-primary px-2 py-1 text-primary-foreground hover:bg-primary/90"
                  >
                    Keep mine
                  </button>
                </div>
              )}
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
                <span className="text-muted-foreground/45">Start writing…</span>
              )}
            </div>
          )}
        </div>
      </div>
      {!collapsed && (
        <div className="ml-[1.4rem] border-l border-primary/10 pl-3">
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
          : "bg-primary/65";
  return (
    <span
      className={`mt-[0.55rem] size-2 shrink-0 rounded-full ring-4 ring-transparent transition group-hover:ring-primary/8 ${cls}`}
    />
  );
}
