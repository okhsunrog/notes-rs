import { useCallback, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createNote,
  deletePage,
  getContainingPage,
  getHistoryStatus,
  getNodeByUuid,
  redo,
  undo,
  type Node,
  type SearchHit,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";

export function useNotesWorkspace(ready: boolean, onStatus: (message: string) => void) {
  const queryClient = useQueryClient();
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [activePageUuid, setActivePageUuid] = useState<string | null>(null);
  const [newNote, setNewNote] = useState<{ pageUuid: string; blockId: number | null } | null>(null);
  const [creatingNote, setCreatingNote] = useState(false);
  const creatingNoteRef = useRef(false);

  const historyQuery = useQuery({
    queryKey: queryKeys.history,
    queryFn: getHistoryStatus,
    enabled: ready,
  });
  const activeNodeQuery = useQuery({
    queryKey: queryKeys.node(activePageUuid ?? "inactive"),
    queryFn: () => getNodeByUuid(activePageUuid as string),
    enabled: activePageUuid !== null,
  });
  const activeNode = activeNodeQuery.data ?? null;

  const createNewNote = useCallback(async () => {
    if (creatingNoteRef.current) return;
    creatingNoteRef.current = true;
    setCreatingNote(true);
    try {
      const note = await createNote();
      queryClient.setQueryData(queryKeys.node(note.page.uuid), note.page);
      queryClient.setQueryData(queryKeys.children(note.page.uuid), [note.initialBlock]);
      await queryClient.invalidateQueries({ queryKey: queryKeys.pages });
      setNewNote({ pageUuid: note.page.uuid, blockId: note.initialBlock.id });
      setActivePageUuid(note.page.uuid);
      setHits([]);
      onStatus("New note ready — name it, then press Enter to write.");
      window.dispatchEvent(new Event("notes-rs:show-main"));
    } catch (error) {
      onStatus(`create error: ${String(error)}`);
    } finally {
      creatingNoteRef.current = false;
      setCreatingNote(false);
    }
  }, [onStatus, queryClient]);

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
    (updated: Node) => {
      setHits((current) =>
        current.map((hit) => (hit.node.uuid === updated.uuid ? { ...hit, node: updated } : hit)),
      );
      queryClient.setQueryData(queryKeys.node(updated.uuid), updated);
      void queryClient.invalidateQueries({ queryKey: queryKeys.pages });
    },
    [queryClient],
  );

  const openNode = useCallback(
    async (node: Node) => {
      try {
        const page = node.kind === "page" ? node : await getContainingPage(node.id);
        if (!page) {
          onStatus(`No containing page found for #${node.id}`);
          return;
        }
        queryClient.setQueryData(queryKeys.node(page.uuid), page);
        setActivePageUuid(page.uuid);
        window.dispatchEvent(new Event("notes-rs:show-main"));
        if (page.uuid !== node.uuid) {
          onStatus(`Opened ${page.title ?? "page"} containing #${node.id}`);
        }
      } catch (error) {
        onStatus(`open error: ${String(error)}`);
      }
    },
    [onStatus, queryClient],
  );

  const removePage = useCallback(
    async (node: Node) => {
      if (
        !window.confirm(
          `Delete “${node.title ?? "untitled"}” and all of its blocks? A backup will be created first.`,
        )
      )
        return;
      try {
        if (await deletePage(node.id)) {
          setActivePageUuid(null);
          queryClient.removeQueries({ queryKey: queryKeys.node(node.uuid), exact: true });
          setHits((current) => current.filter((hit) => hit.node.uuid !== node.uuid));
          onStatus("Page deleted; a recovery backup was created.");
        }
      } catch (error) {
        onStatus(`delete error: ${String(error)}`);
      }
    },
    [onStatus, queryClient],
  );

  const selectPage = useCallback(
    (page: Node) => {
      setNewNote(null);
      queryClient.setQueryData(queryKeys.node(page.uuid), page);
      setActivePageUuid(page.uuid);
      window.dispatchEvent(new Event("notes-rs:show-main"));
    },
    [queryClient],
  );

  const resetWorkspace = useCallback(() => {
    setActivePageUuid(null);
    setHits([]);
    void queryClient.invalidateQueries({ queryKey: queryKeys.root });
  }, [queryClient]);

  return {
    activeNode,
    activePageUuid,
    applyUpdated,
    closePage: () => setActivePageUuid(null),
    createNewNote,
    creatingNote,
    history: historyQuery.data ?? ([0, 0] as const),
    hits,
    moveHistory,
    newNote,
    openNode,
    removePage,
    resetWorkspace,
    selectPage,
    setHits,
  };
}
