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
import { notifyError, notifyInfo } from "@/lib/notify";
import {
  CommandFailure,
  deleteBlock,
  getBlock,
  indentBlock,
  moveBlockDown,
  moveBlockUp,
  outdentBlock,
  searchBlocksFts,
  searchPagesByTitle,
  splitBlock,
  setBlockContent,
  setBlockStyle,
  setTaskState,
  type Block,
  type BlockContent,
  type BlockStyle,
  type PageRenderSnapshot,
  type TaskState,
} from "@/lib/api";
import { Select, SelectContent, SelectItem, SelectTrigger } from "@/components/ui/select";
import { BlockEdit, type BlockEditHandle } from "./block-edit";
import {
  resolveBlockEditKey,
  runStructuralEditAfterFlush,
  splitEditorContent,
  type BlockEditKeyEvent,
  type BlockEditPasteEvent,
} from "./block-edit-model";
import { useOutliner } from "./outliner-store";
import { nextSibling, prevSibling } from "./keyboard";
import { RenderedBlock } from "./rendered-block";
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
import { useBlockDraftOverlay, usePageSessionRegistry } from "@/features/pages/page-session";
import {
  BLOCK_STYLE_OPTIONS,
  TASK_STATE_OPTIONS,
  blockStyleForKind,
  blockStylesEqual,
  getBlockStyleOption,
  getTaskStateOption,
  isBlockStyleKind,
  isTaskState,
  replaceCachedBlock,
  type BlockStyleIcon,
} from "./block-style";

type Props = {
  block: Block;
  depth: number;
  ordinal: number;
  measureRef?: (element: HTMLLIElement | null) => void;
  virtualIndex?: number;
  style?: React.CSSProperties;
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

export function BlockNode({ block, depth, ordinal, measureRef, virtualIndex, style }: Props) {
  const store = useOutliner();
  const queryClient = useQueryClient();
  const sessions = usePageSessionRegistry();
  const overlay = useBlockDraftOverlay(block.pageUuid, block.uuid);
  const effectiveMarkdown = overlay?.draft ?? block.markdown;
  const editing = !store.readOnly && store.editingUuid === block.uuid;
  const outline = store.layout === "outline";
  const readOnly = store.readOnly;
  const containerUuid = block.parentUuid ?? block.pageUuid;
  const collapsed = store.isCollapsed(block.uuid);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [styleBusy, setStyleBusy] = useState(false);
  const styleBusyRef = useRef(false);

  const draftRef = useRef(effectiveMarkdown);
  const blockRef = useRef(block);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveInFlight = useRef<Promise<boolean> | null>(null);
  const editRef = useRef<BlockEditHandle>(null);
  const [draftLen, setDraftLen] = useState(effectiveMarkdown.length);
  const longBlock = (editing ? draftLen : effectiveMarkdown.length) >= LONG_BLOCK_CHARS;

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
      sessions.acceptBlockSnapshot(block.pageUuid, block.uuid, {
        text: block.markdown,
        revision: block.markdownRevision,
      });
      const draft =
        sessions.getSnapshot(block.pageUuid).blocks[block.uuid]?.draft ?? block.markdown;
      draftRef.current = draft;
      setDraftLen(draft.length);
      return;
    }
    const decision = reconcileRemoteDraft(previous.markdown, draftRef.current, block.markdown);
    blockRef.current = block;
    sessions.acceptBlockSnapshot(block.pageUuid, block.uuid, {
      text: block.markdown,
      revision: block.markdownRevision,
    });
    const currentOverlay = sessions.getSnapshot(block.pageUuid).blocks[block.uuid];
    if (currentOverlay) {
      draftRef.current = currentOverlay.draft;
      setDraftLen(currentOverlay.draft.length);
      if (currentOverlay.conflict) clearTimer();
      return;
    }
    draftRef.current = block.markdown;
    setDraftLen(block.markdown.length);
    if (decision === "accept_remote" && editing) store.setEditing(null);
  }, [block, editing, sessions, store]);

  useEffect(() => {
    if (!overlay) {
      if (saveState !== "error") setSaveState("idle");
      return;
    }
    draftRef.current = overlay.draft;
    setDraftLen(overlay.draft.length);
    if (overlay.conflict) {
      clearTimer();
      setSaveState("error");
    } else if (overlay.inFlight) {
      setSaveState("saving");
    } else if (saveState !== "error") {
      setSaveState("dirty");
    }
  }, [overlay, saveState]);

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
      queryClient.setQueryData<PageRenderSnapshot>(
        queryKeys.pageRender(updated.pageUuid),
        (snapshot) =>
          snapshot
            ? {
                ...snapshot,
                document: {
                  ...snapshot.document,
                  blocks: snapshot.document.blocks.map((candidate) =>
                    candidate.uuid === updated.uuid ? updated : candidate,
                  ),
                },
              }
            : snapshot,
      );
    },
    [containerUuid, queryClient],
  );

  const flush = useCallback(async (): Promise<boolean> => {
    clearTimer();
    if (saveInFlight.current) return saveInFlight.current;

    const pending = (async () => {
      while (true) {
        const current = blockRef.current;
        const attempt = sessions.beginBlockSave(current.pageUuid, current.uuid);
        if (!attempt) {
          const currentOverlay = sessions.getSnapshot(current.pageUuid).blocks[current.uuid];
          if (currentOverlay?.conflict || currentOverlay?.inFlight) {
            setSaveState(currentOverlay.conflict ? "error" : "saving");
            return false;
          }
          setSaveState("idle");
          return true;
        }
        setSaveState("saving");
        try {
          const updated = await setBlockContent(
            current.uuid,
            blockContent(attempt.draft),
            attempt.expectedRevision,
          );
          applyBlockSnapshot(updated);
          sessions.acknowledgeBlockSave(current.pageUuid, current.uuid, attempt, {
            text: updated.markdown,
            revision: updated.markdownRevision,
          });
          if (sessions.getSnapshot(current.pageUuid).blocks[current.uuid]) continue;
          setSaveState("idle");
          return true;
        } catch (err) {
          sessions.failBlockSave(current.pageUuid, current.uuid, attempt);
          if (err instanceof CommandFailure && err.code === "conflict") {
            try {
              const latest = await getBlock(current.uuid);
              if (latest) {
                applyBlockSnapshot(latest);
                sessions.acceptBlockSnapshot(current.pageUuid, current.uuid, {
                  text: latest.markdown,
                  revision: latest.markdownRevision,
                });
              }
            } catch (refreshError) {
              console.error("block conflict refresh failed", refreshError);
            }
          }
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
  }, [applyBlockSnapshot, sessions]);

  const changeBlockStyle = async (value: string) => {
    if (!isBlockStyleKind(value) || styleBusyRef.current || readOnly) return;
    const nextStyle = blockStyleForKind(value, blockRef.current.style);
    if (blockStylesEqual(nextStyle, blockRef.current.style)) return;

    styleBusyRef.current = true;
    setStyleBusy(true);
    try {
      if (!(await flush())) return;
      const current = blockRef.current;
      const updated = await setBlockStyle(current.uuid, nextStyle);
      applyBlockSnapshot(updated);
      setSaveState("idle");
    } catch (error) {
      console.error("block style update failed", error);
      setSaveState("error");
      notifyError("block style", error);
    } finally {
      styleBusyRef.current = false;
      setStyleBusy(false);
    }
  };

  const changeTaskState = async (value: string) => {
    if (!isTaskState(value) || styleBusyRef.current || readOnly) return;
    const current = blockRef.current;
    if (current.style.kind !== "task" || current.style.state === value) return;

    styleBusyRef.current = true;
    setStyleBusy(true);
    try {
      if (!(await flush())) return;
      const updated = await setTaskState(current.uuid, value);
      applyBlockSnapshot(updated);
      setSaveState("idle");
    } catch (error) {
      console.error("task state update failed", error);
      setSaveState("error");
      notifyError("task state", error);
    } finally {
      styleBusyRef.current = false;
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
          const blocks = trigger.query ? await searchBlocksFts(trigger.query, 8, "prefix") : [];
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
    const changed = draftRef.current !== value;
    draftRef.current = value;
    if (changed) {
      const current = blockRef.current;
      sessions.editBlock(current.pageUuid, current.uuid, value, {
        text: current.markdown,
        revision: current.markdownRevision,
      });
      setDraftLen(value.length);
      setSaveState("dirty");
      clearTimer();
      timer.current = setTimeout(() => {
        timer.current = null;
        void flush();
      }, AUTOSAVE_MS);
    }
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
    if (!overlay?.conflict) return;
    clearTimer();
    sessions.useRemoteBlock(block.pageUuid, block.uuid);
    draftRef.current = blockRef.current.markdown;
    setDraftLen(blockRef.current.markdown.length);
    setSaveState("idle");
    store.setEditing(null);
  };

  const splitExpectedRevision = () =>
    sessions.getSnapshot(block.pageUuid).blocks[block.uuid]?.baseRevision ??
    blockRef.current.markdownRevision;

  const keepLocalVersion = () => {
    if (!overlay?.conflict) return;
    sessions.keepLocalBlock(block.pageUuid, block.uuid);
    void flush();
  };

  const onPaste = (event: BlockEditPasteEvent) => {
    if (event.isComposing || !event.text) return false;
    const paragraphs = event.text
      .split(/\n[ \t]*(?:\n[ \t]*)+/)
      .map((p) => p.trim())
      .filter((p) => p.length > 0);
    if (paragraphs.length < 2) return false; // single paragraph → native CM paste
    closeAutocomplete();

    // Splice paragraph 0 into the current block at the caret.
    editRef.current?.replaceRange(event.selectionStart, event.selectionEnd, paragraphs[0]);

    clearTimer();
    void (async () => {
      try {
        const completed = await runStructuralEditAfterFlush(flush, async () => {
          const parts = [draftRef.current, ...paragraphs.slice(1)].map(blockContent);
          const changed = await splitBlock(block.uuid, parts, splitExpectedRevision());
          sessions.discardBlock(block.pageUuid, block.uuid);
          await invalidateChildren(containerUuid);
          store.setEditing(lastOrderedBlock(changed)?.uuid ?? block.uuid);
        });
        if (!completed) return;
      } catch (error) {
        console.error("paste split failed", error);
        setSaveState("error");
        return;
      }

      if (!localStorage.getItem("outliner.paste-split.notified")) {
        notifyInfo("Each paragraph became a separate block.");
        localStorage.setItem("outliner.paste-split.notified", "1");
      }
    })();
    return true;
  };

  /** Split this block's full content on blank-line paragraph breaks. Used by
   * the long-block info-icon nudge. If the content has no blank-line breaks,
   * we surface guidance instead of silently doing nothing. */
  const onSplitCurrent = async () => {
    const source = editing ? draftRef.current : effectiveMarkdown;
    const paragraphs = source
      .split(/\n[ \t]*(?:\n[ \t]*)+/)
      .map((p) => p.trim())
      .filter((p) => p.length > 0);
    if (paragraphs.length < 2) {
      notifyInfo("No paragraph breaks found — add blank lines between paragraphs first.");
      return;
    }
    clearTimer();
    try {
      const changed = await splitBlock(
        block.uuid,
        paragraphs.map(blockContent),
        splitExpectedRevision(),
      );
      sessions.discardBlock(block.pageUuid, block.uuid);
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
    if (sessions.getSnapshot(block.pageUuid).blocks[block.uuid]?.conflict) {
      setSaveState("error");
      return;
    }
    clearTimer();
    try {
      await runStructuralEditAfterFlush(flush, async () => {
        const parts = splitEditorContent(draftRef.current, selectionStart, selectionEnd).map(
          blockContent,
        );
        const changed = await splitBlock(block.uuid, parts, splitExpectedRevision());
        sessions.discardBlock(block.pageUuid, block.uuid);
        await invalidateChildren(containerUuid);
        store.setEditing(
          changed.find((candidate) => candidate.uuid !== block.uuid)?.uuid ?? block.uuid,
        );
      });
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
      sessions.discardBlock(block.pageUuid, block.uuid);
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
    store.toggleCollapsed(block.uuid);
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

  const onKeyDown = (event: BlockEditKeyEvent) => {
    const resolution = resolveBlockEditKey(event, {
      autocompleteOpen: trigger !== null,
      autocompleteHasItems: acItems.length > 0,
      draftEmpty: draftRef.current.length === 0,
    });
    if (resolution.closeAutocomplete) closeAutocomplete();

    switch (resolution.action) {
      case "native":
        return false;
      case "next-autocomplete":
        setAcIdx((i) => (acItems.length ? (i + 1) % acItems.length : 0));
        return true;
      case "previous-autocomplete":
        setAcIdx((i) => (acItems.length ? (i - 1 + acItems.length) % acItems.length : 0));
        return true;
      case "accept-autocomplete":
        acceptAutocomplete(acIdx);
        return true;
      case "close-autocomplete":
        closeAutocomplete();
        return true;
      case "collapse":
        toggleCollapsed();
        return true;
      case "reorder-up":
        void onReorder("up");
        return true;
      case "reorder-down":
        void onReorder("down");
        return true;
      case "split":
        void onEnter(event.selectionStart, event.selectionEnd);
        return true;
      case "delete-empty":
        void onBackspaceEmpty();
        return true;
      case "indent":
        void onTab();
        return true;
      case "outdent":
        void onShiftTab();
        return true;
      case "move-previous":
        onUpDown("up");
        return true;
      case "move-next":
        onUpDown("down");
        return true;
      case "stop-editing":
        void flush();
        store.setEditing(null);
        return true;
    }
  };

  return (
    <li
      ref={measureRef}
      style={style}
      className="flex flex-col"
      // TanStack Virtual reads this attribute inside measureElement. Without
      // it dynamic rows keep their estimates, producing gaps or overlaps.
      data-index={virtualIndex}
      data-block-depth={depth}
      data-block-style={block.style.kind}
      data-task-state={block.style.kind === "task" ? block.style.state : undefined}
    >
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
              className={cn(
                "mt-1.5 flex size-4 shrink-0 items-center justify-center rounded text-muted-foreground/35 transition hover:bg-primary/10 hover:text-primary",
                !store.hasChildren(block.uuid) && "invisible",
              )}
              onClick={toggleCollapsed}
              disabled={!store.hasChildren(block.uuid)}
              title={collapsed ? "expand" : "collapse"}
            >
              {collapsed ? (
                <ChevronRight className="size-3" />
              ) : (
                <ChevronDown className="reveal-on-hover size-3" />
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
              draftRef.current = effectiveMarkdown;
              store.setEditing(block.uuid);
            }
          }}
        >
          {editing ? (
            <>
              <BlockEdit
                ref={editRef}
                initial={effectiveMarkdown}
                onChange={onDraftChange}
                onBlur={onBlur}
                onKeyDown={onKeyDown}
                onPaste={onPaste}
                autoFocus
              />
              {overlay?.conflict && (
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
              <RenderedBlock
                block={
                  effectiveMarkdown === block.markdown
                    ? block
                    : { ...block, markdown: effectiveMarkdown }
                }
                ordinal={ordinal}
                layout={store.layout}
                readOnly={readOnly}
                onOpenLink={store.onOpenMarkdownLink}
                resolveImage={store.resolveImage}
                taskBusy={styleBusy}
                onTaskStateChange={(state) => void changeTaskState(state)}
              />
            </div>
          )}
        </div>
        {block.style.kind === "task" && !readOnly && (
          <TaskStatePicker
            state={block.style.state}
            editing={editing}
            busy={styleBusy}
            onChange={(value) => void changeTaskState(value)}
            onRestoreEditorFocus={() => editRef.current?.focus()}
          />
        )}
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
      value={style.kind}
      open={open}
      onOpenChange={setOpen}
      onValueChange={(value) => {
        if (value !== null) onChange(value);
      }}
      disabled={busy}
    >
      <SelectTrigger
        size="sm"
        aria-label={`Block style: ${current.label}`}
        title={`Block style: ${current.label}`}
        className={cn(
          "mt-0.5 h-7 min-w-0 shrink-0 gap-1 rounded-lg border-transparent bg-transparent px-1.5 shadow-none hover:border-border/70 hover:surface-card focus-visible:border-border focus-visible:ring-2 [&>svg:last-child]:size-3",
          editing || open ? "opacity-100" : "reveal-on-hover transition-opacity",
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
        className="min-w-48 rounded-xl border-border/70 p-1 shadow-popover"
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

function TaskStatePicker({
  state,
  editing,
  busy,
  onChange,
  onRestoreEditorFocus,
}: {
  state: TaskState;
  editing: boolean;
  busy: boolean;
  onChange: (value: string) => void;
  onRestoreEditorFocus: () => void;
}) {
  const [open, setOpen] = useState(false);
  const current = getTaskStateOption(state);

  return (
    <Select
      value={state}
      open={open}
      onOpenChange={setOpen}
      onValueChange={(value) => {
        if (value !== null) onChange(value);
      }}
      disabled={busy}
    >
      <SelectTrigger
        size="sm"
        aria-label={`Task state: ${current.label}`}
        title={`Task state: ${current.label}`}
        className="mt-0.5 h-7 min-w-16 shrink-0 rounded-lg border-primary/15 bg-primary/5 px-2 text-[10px] font-semibold tracking-wide text-primary uppercase shadow-none hover:bg-primary/10 focus-visible:ring-2"
      >
        {busy ? <Loader2 className="size-3 animate-spin" /> : <span>{current.label}</span>}
      </SelectTrigger>
      <SelectContent
        position="popper"
        align="end"
        sideOffset={4}
        className="min-w-40 rounded-xl border-border/70 p-1 shadow-popover"
        onCloseAutoFocus={(event) => {
          if (!editing) return;
          event.preventDefault();
          onRestoreEditorFocus();
        }}
      >
        {TASK_STATE_OPTIONS.map((option) => (
          <SelectItem key={option.value} value={option.value} className="rounded-lg py-2">
            {option.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
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
          : "bg-primary/65";
  return (
    <span
      className={`mt-[0.55rem] size-2 shrink-0 rounded-full ring-4 ring-transparent transition group-hover:ring-primary/8 ${cls}`}
    />
  );
}
