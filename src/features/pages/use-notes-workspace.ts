import { useCallback, useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { MarkdownOpenRequest } from "@/features/markdown";
import {
  createNote,
  deletePage,
  getBlock,
  getContainingPage,
  getHistoryStatus,
  getPage,
  getPageByTitle,
  redo,
  undo,
  type Content,
  type Page,
  type SearchHit,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { useConfirmation } from "@/app/confirmation";

export function useNotesWorkspace(
  ready: boolean,
  onStatus: (message: string) => void,
  showEditor: () => void,
) {
  const confirm = useConfirmation();
  const queryClient = useQueryClient();
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [activePageUuid, setActivePageUuid] = useState<string | null>(null);
  const [newNote, setNewNote] = useState<{ pageUuid: string; blockUuid: string | null } | null>(
    null,
  );
  const [creatingNote, setCreatingNote] = useState(false);
  const creatingNoteRef = useRef(false);

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

  useEffect(() => {
    if (activePageUuid !== null && activePageQuery.isSuccess && activePageQuery.data === null) {
      setActivePageUuid(null);
      setNewNote(null);
    }
  }, [activePageQuery.data, activePageQuery.isSuccess, activePageUuid]);

  const createNewNote = useCallback(async () => {
    if (creatingNoteRef.current) return;
    creatingNoteRef.current = true;
    setCreatingNote(true);
    try {
      const note = await createNote();
      queryClient.setQueryData(queryKeys.page(note.page.uuid), note.page);
      queryClient.setQueryData(queryKeys.children(note.page.uuid), [note.initialBlock]);
      await queryClient.invalidateQueries({ queryKey: queryKeys.pages });
      setNewNote({ pageUuid: note.page.uuid, blockUuid: note.initialBlock.uuid });
      setActivePageUuid(note.page.uuid);
      setHits([]);
      onStatus("New note ready — name it, then press Enter to write.");
      showEditor();
    } catch (error) {
      onStatus(`create error: ${String(error)}`);
    } finally {
      creatingNoteRef.current = false;
      setCreatingNote(false);
    }
  }, [onStatus, queryClient, showEditor]);

  const moveHistory = useCallback(
    async (direction: "undo" | "redo") => {
      try {
        const changed = direction === "undo" ? await undo() : await redo();
        if (changed) {
          setActivePageUuid(null);
          setHits([]);
          onStatus(direction === "undo" ? "Undid structural change." : "Redid structural change.");
          await queryClient.invalidateQueries({ queryKey: queryKeys.root });
        }
      } catch (error) {
        onStatus(`${direction} error: ${String(error)}`);
      }
    },
    [onStatus, queryClient],
  );

  const applyUpdated = useCallback(
    (updated: Page) => {
      setHits((current) =>
        current.map((hit) =>
          hit.content.kind === "page" && hit.content.record.uuid === updated.uuid
            ? { ...hit, content: { kind: "page", record: updated } }
            : hit,
        ),
      );
      queryClient.setQueryData(queryKeys.page(updated.uuid), updated);
      void queryClient.invalidateQueries({ queryKey: queryKeys.pages });
    },
    [queryClient],
  );

  const openContent = useCallback(
    async (content: Content) => {
      try {
        const page =
          content.kind === "page" ? content.record : await getContainingPage(content.record.uuid);
        if (!page) {
          onStatus(`No containing page found for ${content.record.uuid}`);
          return;
        }
        queryClient.setQueryData(queryKeys.page(page.uuid), page);
        setActivePageUuid(page.uuid);
        showEditor();
        if (content.kind === "block") {
          onStatus(`Opened ${page.title ?? "page"} containing the selected block.`);
        }
      } catch (error) {
        onStatus(`open error: ${String(error)}`);
      }
    },
    [onStatus, queryClient, showEditor],
  );

  const openMarkdownLink = useCallback(
    async (request: MarkdownOpenRequest) => {
      const { disposition, target } = request;
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
            onStatus(`Section #${fragment || target.fragment} is not available on this page.`);
          }
          return;
        }

        const content: Content | null =
          target.kind === "page"
            ? await getPageByTitle(target.title).then((page) =>
                page ? { kind: "page", record: page } : null,
              )
            : await getBlock(target.uuid).then((block) =>
                block ? { kind: "block", record: block } : null,
              );
        if (!content) {
          onStatus(
            target.kind === "page"
              ? `Page “${target.title}” was not found.`
              : `Block ${target.uuid} was not found.`,
          );
          return;
        }

        await openContent(content);
        if (disposition === "adjacent") {
          onStatus("Split view is not available yet; opened the link in the current pane.");
        }
      } catch (error) {
        onStatus(`link error: ${String(error)}`);
      }
    },
    [onStatus, openContent],
  );

  const removePage = useCallback(
    async (page: Page) => {
      if (
        !(await confirm({
          title: "Delete note?",
          description: `“${page.title ?? "Untitled"}” and all of its blocks will be removed. A recovery backup is created first.`,
          confirmLabel: "Delete note",
          destructive: true,
        }))
      )
        return;
      try {
        if (await deletePage(page.uuid)) {
          setActivePageUuid(null);
          queryClient.removeQueries({ queryKey: queryKeys.page(page.uuid), exact: true });
          setHits((current) =>
            current.filter(
              (hit) => !(hit.content.kind === "page" && hit.content.record.uuid === page.uuid),
            ),
          );
          onStatus("Page deleted; a recovery backup was created.");
        }
      } catch (error) {
        onStatus(`delete error: ${String(error)}`);
      }
    },
    [confirm, onStatus, queryClient],
  );

  const selectPage = useCallback(
    (page: Page) => {
      setNewNote(null);
      queryClient.setQueryData(queryKeys.page(page.uuid), page);
      setActivePageUuid(page.uuid);
      showEditor();
    },
    [queryClient, showEditor],
  );

  const resetWorkspace = useCallback(() => {
    setActivePageUuid(null);
    setHits([]);
    void queryClient.invalidateQueries({ queryKey: queryKeys.root });
  }, [queryClient]);

  return {
    activePage,
    activePageUuid,
    applyUpdated,
    closePage: () => setActivePageUuid(null),
    createNewNote,
    creatingNote,
    history: historyQuery.data ?? { undoCount: 0, redoCount: 0 },
    hits,
    moveHistory,
    newNote,
    openMarkdownLink,
    openContent,
    removePage,
    resetWorkspace,
    selectPage,
    setHits,
  };
}

function decodeFragment(fragment: string): string {
  try {
    return decodeURIComponent(fragment);
  } catch {
    return fragment;
  }
}
