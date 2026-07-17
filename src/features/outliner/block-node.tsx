import { useCallback, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronRight,
  CodeXml,
  Heading1,
  Heading2,
  Heading3,
  Info,
  List,
  ListOrdered,
  ListTodo,
  Loader2,
  Minus,
  Pilcrow,
  Quote,
  type LucideIcon,
} from "lucide-react";
import { toast } from "sonner";
import {
  deleteBlock,
  indentBlock,
  moveBlockDown,
  moveBlockUp,
  outdentBlock,
  searchBlocksFts,
  searchPagesByTitle,
  splitBlock,
  setBlockContent,
  setBlockStyle,
  type Block,
  type BlockContent,
  type BlockStyle,
  type PageLayout,
} from "@/lib/api";
import { Select, SelectContent, SelectItem, SelectTrigger } from "@/components/ui/select";
import { BlockChildren } from "./block-tree";
import { BlockEdit, type BlockEditHandle } from "./block-edit";
import { useOutliner } from "./outliner-store";
import { nextSibling, prevSibling } from "./keyboard";
import { renderMarkdown } from "./render-markdown";
import { detectTrigger, type Trigger } from "./autocomplete";
import {
  AutocompleteMenu,
  blockToItem,
  pageToItem,
  type AutocompleteItem,
} from "./autocomplete-menu";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import { reconcileRemoteDraft } from "./editor-sync";
import {
  BLOCK_STYLE_OPTIONS,
  getBlockStyleOption,
  isBlockStyle,
  replaceCachedBlock,
  type BlockStyleIcon,
} from "./block-style";

type Props = {
  block: Block;
  depth: number;
  ordinal: number;
};

type SaveState = "idle" | "dirty" | "saving" | "error";

const AUTOSAVE_MS = 400;
const AC_DEBOUNCE_MS = 120;
const LONG_BLOCK_CHARS = 600;

function blockContent(markdown: string): BlockContent {
  return { markdown };
}

function lastOrderedBlock(blocks: Block[]) {
  return blocks.reduce<Block | null>(
    (last, candidate) => (!last || candidate.orderKey > last.orderKey ? candidate : last),
    null,
  );
}

export function BlockNode({ block, depth, ordinal }: Props) {
  const store = useOutliner();
  const queryClient = useQueryClient();
  const editing = !store.readOnly && store.editingUuid === block.uuid;
  const outline = store.layout === "outline";
  const readOnly = store.readOnly;
  const containerUuid = block.parentUuid ?? block.pageUuid;
  const collapseKey = `outliner.collapsed.${block.uuid}`;
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem(collapseKey) === "1");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [styleBusy, setStyleBusy] = useState(false);

  const draftRef = useRef(block.markdown);
  const blockRef = useRef(block);
  const remoteConflictRef = useRef<Block | null>(null);
  const [remoteConflict, setRemoteConflict] = useState<Block | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveInFlight = useRef<Promise<boolean> | null>(null);
  const editRef = useRef<BlockEditHandle>(null);
  const [draftLen, setDraftLen] = useState(block.markdown.length);
  const longBlock = (editing ? draftLen : block.markdown.length) >= LONG_BLOCK_CHARS;

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
      draftRef.current = block.markdown;
      setDraftLen(block.markdown.length);
      remoteConflictRef.current = null;
      setRemoteConflict(null);
      return;
    }
    const decision = reconcileRemoteDraft(previous.markdown, draftRef.current, block.markdown);
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
    draftRef.current = block.markdown;
    setDraftLen(block.markdown.length);
    if (editing) store.setEditing(null);
  }, [block, editing, store]);

  const siblings = () => queryClient.getQueryData<Block[]>(queryKeys.children(containerUuid)) ?? [];

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

  const applyBlockSnapshot = useCallback(
    (updated: Block) => {
      blockRef.current = updated;
      queryClient.setQueryData<Block[]>(queryKeys.children(containerUuid), (rows = []) =>
        replaceCachedBlock(rows, updated),
      );
      queryClient.setQueryData(queryKeys.block(updated.uuid), updated);
    },
    [containerUuid, queryClient],
  );

  const flush = useCallback(async (): Promise<boolean> => {
    clearTimer();
    if (saveInFlight.current) return saveInFlight.current;

    const pending = (async () => {
      while (true) {
        if (remoteConflictRef.current) {
          setSaveState("error");
          return false;
        }
        const current = blockRef.current;
        const next = draftRef.current;
        if (next === current.markdown) {
          setSaveState("idle");
          return true;
        }
        setSaveState("saving");
        try {
          const updated = await setBlockContent(current.uuid, blockContent(next));
          applyBlockSnapshot(updated);
          // The user may have typed while this save was in flight. Persist that
          // newer draft in the same serialized save loop before reporting clean.
          if (draftRef.current !== updated.markdown) continue;
          setSaveState("idle");
          return true;
        } catch (err) {
          console.error("block save failed", err);
          setSaveState("error");
          return false;
        }
      }
    })();
    saveInFlight.current = pending;
    try {
      return await pending;
    } finally {
      if (saveInFlight.current === pending) saveInFlight.current = null;
    }
  }, [applyBlockSnapshot]);

  const changeBlockStyle = async (value: string) => {
    if (!isBlockStyle(value) || styleBusy || readOnly) return;
    if (value === blockRef.current.style) return;

    setStyleBusy(true);
    try {
      if (!(await flush())) return;
      const current = blockRef.current;
      const updated = await setBlockStyle(current.uuid, value);
      applyBlockSnapshot(updated);
      setSaveState("idle");
    } catch (error) {
      console.error("block style update failed", error);
      setSaveState("error");
      toast.error("Could not change the block style.");
    } finally {
      setStyleBusy(false);
    }
  };

  // Re-fetch results whenever the trigger query changes.
  useEffect(() => {
    if (!trigger) return;
    if (acDebounce.current) clearTimeout(acDebounce.current);
    const reqId = ++acReqId.current;
    setAcLoading(true);
    acDebounce.current = setTimeout(async () => {
      try {
        if (trigger.kind === "[[") {
          const pages = await searchPagesByTitle(trigger.query, 8);
          if (reqId !== acReqId.current) return;
          setAcItems(pages.map(pageToItem));
        } else {
          const ftsQuery = buildFtsPrefix(trigger.query);
          const blocks = ftsQuery ? await searchBlocksFts(ftsQuery, 8) : [];
          if (reqId !== acReqId.current) return;
          setAcItems(blocks.map(blockToItem));
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
    draftRef.current = remote.markdown;
    setDraftLen(remote.markdown.length);
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
      const changed = await splitBlock(block.uuid, parts);
      await invalidateChildren(containerUuid);
      store.setEditing(lastOrderedBlock(changed)?.uuid ?? block.uuid);
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
    const source = editing ? draftRef.current : block.markdown;
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
      const changed = await splitBlock(block.uuid, paragraphs.map(blockContent));
      draftRef.current = paragraphs[0];
      setDraftLen(paragraphs[0].length);
      await invalidateChildren(containerUuid);
      store.setEditing(lastOrderedBlock(changed)?.uuid ?? block.uuid);
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

  const onEnter = async (selectionStart: number, selectionEnd: number) => {
    if (remoteConflictRef.current) {
      setSaveState("error");
      return;
    }
    clearTimer();
    const markdown = draftRef.current;
    const parts = [markdown.slice(0, selectionStart), markdown.slice(selectionEnd)].map(
      blockContent,
    );
    try {
      const changed = await splitBlock(block.uuid, parts);
      await invalidateChildren(containerUuid);
      store.setEditing(
        changed.find((candidate) => candidate.uuid !== block.uuid)?.uuid ?? block.uuid,
      );
    } catch (e) {
      console.error("enter (split block) failed", e);
      setSaveState("error");
    }
  };

  const onBackspaceEmpty = async () => {
    if (draftRef.current.length > 0) return false;
    const prev = prevSibling(siblings(), block.uuid);
    try {
      const ok = await deleteBlock(block.uuid);
      if (!ok) return false;
      queryClient.setQueryData<Block[]>(queryKeys.children(containerUuid), (rows = []) =>
        rows.filter((row) => row.uuid !== block.uuid),
      );
      if (prev) store.setEditing(prev.uuid);
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
      store.setEditing(moved.uuid);
    } catch (e) {
      console.error("tab indent failed", e);
    }
  };

  const onShiftTab = async () => {
    if (block.parentUuid === null) return;
    await flush();
    try {
      const moved = await outdentBlock(block.uuid);
      await invalidateChildren();
      store.setEditing(moved.uuid);
    } catch (e) {
      console.error("shift-tab outdent failed", e);
    }
  };

  const onUpDown = (dir: "up" | "down") => {
    const rows = siblings();
    const target = dir === "up" ? prevSibling(rows, block.uuid) : nextSibling(rows, block.uuid);
    if (target) store.setEditing(target.uuid);
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
      await invalidateChildren(containerUuid);
      store.setEditing(block.uuid);
    } catch (error) {
      console.error("block reorder failed", error);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return;

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
      void onEnter(e.currentTarget.selectionStart, e.currentTarget.selectionEnd);
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
    <li className="flex flex-col" data-block-style={block.style}>
      <div
        className={`group flex items-start transition-colors ${
          outline
            ? "-mx-2 gap-1 rounded-xl px-2 py-1 hover:bg-primary/[0.035] focus-within:bg-primary/[0.04]"
            : "py-1.5"
        }`}
      >
        {outline && (
          <>
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
          </>
        )}
        {longBlock && !readOnly && (
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
            if (!editing && !readOnly) {
              draftRef.current = block.markdown;
              store.setEditing(block.uuid);
            }
          }}
        >
          {editing ? (
            <>
              <BlockEdit
                ref={editRef}
                initial={block.markdown}
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
            <div className={readOnly ? "cursor-default" : "cursor-text"}>
              <RenderedBlock block={block} ordinal={ordinal} layout={store.layout} />
            </div>
          )}
        </div>
        {!readOnly && (
          <BlockStylePicker
            style={block.style}
            editing={editing}
            busy={styleBusy}
            onChange={(value) => void changeBlockStyle(value)}
            onRestoreEditorFocus={() => editRef.current?.focus()}
          />
        )}
      </div>
      {(!outline || !collapsed) && (
        <div
          className={
            outline
              ? "ml-[1.4rem] border-l border-primary/10 pl-3"
              : depth > 0
                ? "ml-5 border-l border-border/45 pl-4"
                : ""
          }
        >
          <BlockChildren pageUuid={block.pageUuid} parentUuid={block.uuid} depth={depth + 1} />
        </div>
      )}
    </li>
  );
}

const BLOCK_STYLE_ICONS: Record<BlockStyleIcon, LucideIcon> = {
  paragraph: Pilcrow,
  bullet: List,
  numbered: ListOrdered,
  task: ListTodo,
  "heading-1": Heading1,
  "heading-2": Heading2,
  "heading-3": Heading3,
  quote: Quote,
  code: CodeXml,
  divider: Minus,
};

function BlockStylePicker({
  style,
  editing,
  busy,
  onChange,
  onRestoreEditorFocus,
}: {
  style: BlockStyle;
  editing: boolean;
  busy: boolean;
  onChange: (value: string) => void;
  onRestoreEditorFocus: () => void;
}) {
  const [open, setOpen] = useState(false);
  const current = getBlockStyleOption(style);
  const CurrentIcon = BLOCK_STYLE_ICONS[current.icon];

  return (
    <Select
      value={style}
      open={open}
      onOpenChange={setOpen}
      onValueChange={onChange}
      disabled={busy}
    >
      <SelectTrigger
        size="sm"
        aria-label={`Block style: ${current.label}`}
        title={`Block style: ${current.label}`}
        className={cn(
          "mt-0.5 h-7 min-w-0 shrink-0 gap-1 rounded-lg border-transparent bg-transparent px-1.5 shadow-none hover:border-border/70 hover:bg-card/80 focus-visible:border-border focus-visible:ring-2 [&>svg:last-child]:size-3",
          editing || open
            ? "opacity-100"
            : "opacity-0 transition-opacity group-hover:opacity-100 focus:opacity-100",
        )}
      >
        {busy ? (
          <Loader2 className="size-3.5 animate-spin" />
        ) : (
          <CurrentIcon className="size-3.5" />
        )}
        <span className="sr-only">{current.label}</span>
      </SelectTrigger>
      <SelectContent
        position="popper"
        align="end"
        sideOffset={4}
        className="min-w-48 rounded-xl border-border/70 p-1 shadow-xl"
        onCloseAutoFocus={(event) => {
          if (!editing) return;
          event.preventDefault();
          onRestoreEditorFocus();
        }}
      >
        {BLOCK_STYLE_OPTIONS.map((option) => {
          const Icon = BLOCK_STYLE_ICONS[option.icon];
          return (
            <SelectItem key={option.value} value={option.value} className="rounded-lg py-2">
              <Icon className="size-4" />
              <span>{option.label}</span>
            </SelectItem>
          );
        })}
      </SelectContent>
    </Select>
  );
}

function RenderedBlock({
  block,
  ordinal,
  layout,
}: {
  block: Block;
  ordinal: number;
  layout: PageLayout;
}) {
  if (block.style === "divider") return <hr className="my-4 border-border/70" />;
  if (!block.markdown) {
    return <span className="text-sm text-muted-foreground/45">Start writing…</span>;
  }
  if (block.style === "code") {
    return (
      <pre className="overflow-x-auto rounded-xl border border-border/60 bg-muted/55 p-3 text-xs leading-relaxed">
        <code>{block.markdown}</code>
      </pre>
    );
  }

  const content = renderMarkdown(block.markdown);
  if (block.style === "heading_1") {
    return <h2 className="mt-5 mb-2 text-2xl font-semibold tracking-tight">{content}</h2>;
  }
  if (block.style === "heading_2") {
    return <h3 className="mt-4 mb-1.5 text-xl font-semibold tracking-tight">{content}</h3>;
  }
  if (block.style === "heading_3") {
    return <h4 className="mt-3 mb-1 text-base font-semibold">{content}</h4>;
  }
  if (block.style === "quote") {
    return (
      <blockquote className="border-l-2 border-primary/35 pl-4 text-sm leading-relaxed text-muted-foreground italic">
        {content}
      </blockquote>
    );
  }
  if (layout === "outline" && (block.style === "bullet" || block.style === "numbered")) {
    return <p className="whitespace-pre-wrap break-words text-sm leading-relaxed">{content}</p>;
  }
  if (block.style === "bullet" || block.style === "numbered" || block.style === "task") {
    return (
      <div className="flex gap-2 text-sm leading-relaxed">
        {block.style === "task" ? (
          <input type="checkbox" disabled className="mt-1 size-3.5 accent-primary" />
        ) : (
          <span className="w-4 shrink-0 text-right text-muted-foreground">
            {block.style === "numbered" ? `${ordinal}.` : "•"}
          </span>
        )}
        <span className="min-w-0 whitespace-pre-wrap break-words">{content}</span>
      </div>
    );
  }
  return <p className="whitespace-pre-wrap break-words text-sm leading-relaxed">{content}</p>;
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
