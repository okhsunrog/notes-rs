import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { MarkdownOpenRequest } from "@/features/markdown";
import {
  pageDisplayTitle,
  parseJournalDate,
  todayJournalDate,
} from "@/features/journal/journal-date";
import {
  appendToJournal,
  createNote,
  deletePage,
  getBlock,
  getContainingPage,
  getHistoryStatus,
  getJournal,
  getPage,
  getPageByTitle,
  redo,
  undo,
  type Content,
  type JournalDate,
  type Page,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { notifyError, notifyInfo, notifySuccess } from "@/lib/notify";
import { useConfirmation } from "@/app/confirmation";
import {
  PaneContentKind,
  adjacentDisposition,
  currentDisposition,
  getActivePane,
  homeTarget,
  journalDayTarget,
  pageTarget,
  type OpenDisposition,
  type OpenTarget,
} from "@/features/workspace/workspace-model";
import type { WorkspaceController } from "@/features/workspace/workspace-controller";
import { useWorkspaceStore } from "@/features/workspace/workspace-store";
import { usePageSessionRegistry } from "./page-session";

export function useNotesWorkspace(ready: boolean, showEditor: () => void) {
  const confirm = useConfirmation();
  const pageSessions = usePageSessionRegistry();
  const queryClient = useQueryClient();
  const activePane = useWorkspaceStore(getActivePane);
  const dispatchWorkspace = useWorkspaceStore((state) => state.dispatch);
  const [newNote, setNewNote] = useState<{
    pageUuid: string;
    blockUuid: string | null;
    autoFocusTitle: boolean;
  } | null>(null);
  const [creatingNote, setCreatingNote] = useState(false);
  const [journalBusy, setJournalBusy] = useState(false);
  const creatingNoteRef = useRef(false);
  const journalBusyRef = useRef(false);
  const navigationEpochRef = useRef(0);
  const activeContent = activePane.content;
  const activePageUuid =
    activeContent.kind === PaneContentKind.Page
      ? activeContent.pageUuid
      : activeContent.kind === PaneContentKind.Graph
        ? activeContent.focusPageUuid
        : null;
  const pendingJournalDate =
    activeContent.kind === PaneContentKind.JournalDay ? activeContent.date : null;

  const historyQuery = useQuery({
    queryKey: queryKeys.history,
    queryFn: getHistoryStatus,
    enabled: ready,
  });
  const activePageQuery = useQuery({
    queryKey: queryKeys.page(activePageUuid ?? "inactive"),
    queryFn: () => getPage(activePageUuid as string),
    enabled: activePageUuid !== null,
  });
  const activePage = activePageQuery.data ?? null;

  const openTarget = useCallback(
    (target: OpenTarget, disposition: OpenDisposition) => {
      dispatchWorkspace({ type: "open_target", target, disposition });
    },
    [dispatchWorkspace],
  );

  useEffect(() => {
    if (activePageUuid !== null && activePageQuery.isSuccess && activePageQuery.data === null) {
      pageSessions.discardPage(activePageUuid);
      dispatchWorkspace({ type: "forget_page", pageUuid: activePageUuid });
      setNewNote(null);
    }
  }, [
    activePageQuery.data,
    activePageQuery.isSuccess,
    activePageUuid,
    dispatchWorkspace,
    pageSessions,
  ]);

  const createNewNote = useCallback(
    async (title?: string, disposition: OpenDisposition = currentDisposition) => {
      if (creatingNoteRef.current) return;
      const navigationEpoch = ++navigationEpochRef.current;
      creatingNoteRef.current = true;
      setCreatingNote(true);
      try {
        const result = await createNote(title ?? null);
        if (result.status === "existing") {
          if (navigationEpoch !== navigationEpochRef.current) return;
          queryClient.setQueryData(queryKeys.page(result.page.uuid), result.page);
          setNewNote(null);
          openTarget(pageTarget(result.page.uuid), disposition);
          showEditor();
          return;
        }
        const note = result.note;
        queryClient.setQueryData(queryKeys.page(note.page.uuid), note.page);
        queryClient.setQueryData(queryKeys.children(note.page.uuid), [note.initialBlock]);
        await queryClient.invalidateQueries({ queryKey: queryKeys.pages });
        if (navigationEpoch !== navigationEpochRef.current) return;
        setNewNote({
          pageUuid: note.page.uuid,
          blockUuid: note.initialBlock.uuid,
          autoFocusTitle: note.page.title === null,
        });
        openTarget(pageTarget(note.page.uuid, { blockUuid: note.initialBlock.uuid }), disposition);
        if (note.page.title === null) {
          notifyInfo("New note ready — name it, then press Enter to write.");
        }
        showEditor();
      } catch (error) {
        notifyError("create", error);
      } finally {
        creatingNoteRef.current = false;
        setCreatingNote(false);
      }
    },
    [openTarget, queryClient, showEditor],
  );

  const moveHistory = useCallback(
    async (direction: "undo" | "redo") => {
      const navigationEpoch = ++navigationEpochRef.current;
      try {
        const result = direction === "undo" ? await undo() : await redo();
        if (result === "skipped") {
          notifyInfo(
            direction === "undo"
              ? "Undo skipped — the content changed since that action."
              : "Redo skipped — the content changed since that action.",
          );
          return;
        }
        if (result === "applied" && navigationEpoch === navigationEpochRef.current) {
          dispatchWorkspace({ type: "reset" });
          notifyInfo(
            direction === "undo" ? "Undid structural change." : "Redid structural change.",
          );
          await queryClient.invalidateQueries({ queryKey: queryKeys.root });
        }
      } catch (error) {
        notifyError(direction, error);
      }
    },
    [dispatchWorkspace, queryClient],
  );

  const openJournal = useCallback(
    async (date: JournalDate, disposition: OpenDisposition = currentDisposition) => {
      if (journalBusyRef.current) return;
      const navigationEpoch = ++navigationEpochRef.current;
      journalBusyRef.current = true;
      setJournalBusy(true);
      try {
        const page = await queryClient.fetchQuery({
          queryKey: queryKeys.journal(date),
          queryFn: () => getJournal(date),
        });
        if (navigationEpoch !== navigationEpochRef.current) return;
        setNewNote(null);
        if (page) {
          queryClient.setQueryData(queryKeys.page(page.uuid), page);
          openTarget(pageTarget(page.uuid), disposition);
        } else {
          openTarget(journalDayTarget(date), disposition);
        }
        showEditor();
      } catch (error) {
        if (navigationEpoch === navigationEpochRef.current) {
          notifyError("journal", error);
        }
      } finally {
        journalBusyRef.current = false;
        setJournalBusy(false);
      }
    },
    [openTarget, queryClient, showEditor],
  );

  const captureJournal = useCallback(
    async (date: JournalDate, markdown: string, openAfterCapture = false) => {
      if (journalBusyRef.current) return false;
      const navigationEpoch = openAfterCapture ? ++navigationEpochRef.current : null;
      journalBusyRef.current = true;
      setJournalBusy(true);
      try {
        const block = await appendToJournal(date, { markdown }, { kind: "paragraph" });
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: queryKeys.children(block.pageUuid) }),
          queryClient.invalidateQueries({ queryKey: queryKeys.journals }),
          queryClient.invalidateQueries({ queryKey: queryKeys.history }),
        ]);
        if (openAfterCapture) {
          if (navigationEpoch !== navigationEpochRef.current) {
            notifySuccess(`Captured in journal ${date}.`);
            return true;
          }
          const page = await queryClient.fetchQuery({
            queryKey: queryKeys.journal(date),
            queryFn: () => getJournal(date),
          });
          if (!page) throw new Error("captured journal page was not found");
          if (navigationEpoch !== navigationEpochRef.current) {
            notifySuccess(`Captured in journal ${date}.`);
            return true;
          }
          queryClient.setQueryData(queryKeys.page(page.uuid), page);
          setNewNote({ pageUuid: page.uuid, blockUuid: block.uuid, autoFocusTitle: false });
          openTarget(pageTarget(page.uuid, { blockUuid: block.uuid }), currentDisposition);
          showEditor();
        }
        notifySuccess(`Captured in journal ${date}.`);
        return true;
      } catch (error) {
        notifyError("capture", error);
        return false;
      } finally {
        journalBusyRef.current = false;
        setJournalBusy(false);
      }
    },
    [openTarget, queryClient, showEditor],
  );

  const quickCapture = useCallback(
    (markdown: string) => captureJournal(todayJournalDate(), markdown),
    [captureJournal],
  );

  const applyUpdated = useCallback(
    (updated: Page) => {
      queryClient.setQueryData(queryKeys.page(updated.uuid), updated);
      void queryClient.invalidateQueries({ queryKey: queryKeys.pages });
    },
    [queryClient],
  );

  const openContent = useCallback(
    async (content: Content, disposition: OpenDisposition = currentDisposition) => {
      const navigationEpoch = ++navigationEpochRef.current;
      try {
        const page =
          content.kind === "page" ? content.record : await getContainingPage(content.record.uuid);
        if (navigationEpoch !== navigationEpochRef.current) return;
        if (!page) {
          notifyError("open", `No containing page found for ${content.record.uuid}`);
          return;
        }
        queryClient.setQueryData(queryKeys.page(page.uuid), page);
        openTarget(
          pageTarget(page.uuid, {
            blockUuid: content.kind === "block" ? content.record.uuid : null,
          }),
          disposition,
        );
        showEditor();
        if (content.kind === "block") {
          notifyInfo(`Opened ${pageDisplayTitle(page)} containing the selected block.`);
        }
      } catch (error) {
        if (navigationEpoch === navigationEpochRef.current) {
          notifyError("open", error);
        }
      }
    },
    [openTarget, queryClient, showEditor],
  );

  const openMarkdownLink = useCallback(
    async (request: MarkdownOpenRequest) => {
      const { disposition, target } = request;
      const openDisposition = disposition === "adjacent" ? adjacentDisposition : currentDisposition;
      try {
        if (target.kind === "external") {
          await openUrl(target.href);
          return;
        }
        if (target.kind === "fragment") {
          const fragment = decodeFragment(target.fragment);
          const element = fragment ? document.getElementById(fragment) : null;
          if (element) {
            element.scrollIntoView({ behavior: "smooth", block: "start" });
          } else {
            notifyError(
              "open",
              `Section #${fragment || target.fragment} is not available on this page.`,
            );
          }
          return;
        }

        let content: Content | null;
        if (target.kind === "page") {
          const journalDate = parseJournalDate(target.title);
          if (journalDate) {
            await openJournal(journalDate, openDisposition);
            return;
          }
          const page = await getPageByTitle(target.title);
          content = page ? { kind: "page", record: page } : null;
        } else {
          const block = await getBlock(target.uuid);
          content = block ? { kind: "block", record: block } : null;
        }
        if (!content) {
          notifyError(
            "open",
            target.kind === "page"
              ? `Page “${target.title}” was not found.`
              : `Block ${target.uuid} was not found.`,
          );
          return;
        }

        await openContent(content, openDisposition);
      } catch (error) {
        notifyError("link", error);
      }
    },
    [openContent, openJournal],
  );

  const removePage = useCallback(
    async (page: Page) => {
      if (
        !(await confirm({
          title: page.kind.kind === "journal" ? "Delete journal day?" : "Delete note?",
          description: `“${pageDisplayTitle(page)}” and all of its blocks will be removed. A recovery backup is created first.`,
          confirmLabel: page.kind.kind === "journal" ? "Delete journal day" : "Delete note",
          destructive: true,
        }))
      )
        return;
      const navigationEpoch = ++navigationEpochRef.current;
      try {
        if (await deletePage(page.uuid)) {
          if (navigationEpoch !== navigationEpochRef.current) return;
          dispatchWorkspace({ type: "forget_page", pageUuid: page.uuid });
          pageSessions.discardPage(page.uuid);
          queryClient.removeQueries({ queryKey: queryKeys.page(page.uuid), exact: true });
          notifySuccess("Page deleted; a recovery backup was created.");
        }
      } catch (error) {
        notifyError("delete", error);
      }
    },
    [confirm, dispatchWorkspace, pageSessions, queryClient],
  );

  const selectPage = useCallback(
    (page: Page, disposition: OpenDisposition = currentDisposition) => {
      navigationEpochRef.current += 1;
      setNewNote(null);
      queryClient.setQueryData(queryKeys.page(page.uuid), page);
      openTarget(pageTarget(page.uuid), disposition);
      showEditor();
    },
    [openTarget, queryClient, showEditor],
  );

  const resetWorkspace = useCallback(() => {
    navigationEpochRef.current += 1;
    dispatchWorkspace({ type: "reset" });
    void queryClient.invalidateQueries({ queryKey: queryKeys.root });
  }, [dispatchWorkspace, queryClient]);

  const controller = useMemo<WorkspaceController>(
    () => ({
      createNewNote,
      openContent,
      openJournal,
      captureJournal,
      onSaved: applyUpdated,
      onDelete: removePage,
      openMarkdownLink,
    }),
    [
      applyUpdated,
      captureJournal,
      createNewNote,
      openContent,
      openJournal,
      openMarkdownLink,
      removePage,
    ],
  );

  return {
    activePage,
    activePane,
    activePageUuid,
    applyUpdated,
    captureJournal,
    controller,
    closePage: () => {
      navigationEpochRef.current += 1;
      openTarget(homeTarget, currentDisposition);
    },
    createNewNote,
    creatingNote,
    history: historyQuery.data ?? { undoCount: 0, redoCount: 0 },
    moveHistory,
    journalBusy,
    newNote,
    openMarkdownLink,
    openJournal,
    openContent,
    pendingJournalDate,
    removePage,
    resetWorkspace,
    selectPage,
    quickCapture,
  };
}

function decodeFragment(fragment: string): string {
  try {
    return decodeURIComponent(fragment);
  } catch {
    return fragment;
  }
}
