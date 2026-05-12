import { createContext, useContext, useMemo, useRef, useState } from "react";
import { listBlockChildren, type Node } from "@/lib/api";

type ChildrenMap = Map<number, Node[]>;

type Store = {
  /** undefined = not loaded yet; [] = loaded, empty. */
  getChildren: (parentId: number) => Node[] | undefined;
  ensureLoaded: (parentId: number) => void;
  loadError: (parentId: number) => string | undefined;
  replaceBlock: (updated: Node) => void;
  insertAfter: (parentId: number, afterId: number | null, block: Node) => void;
  removeBlock: (parentId: number, id: number) => void;
  /** Local-only move (after a successful server-side moveBlock). */
  moveLocal: (block: Node, oldParentId: number) => void;
  editingId: number | null;
  setEditing: (id: number | null) => void;
};

const OutlinerCtx = createContext<Store | null>(null);

export function useOutliner() {
  const ctx = useContext(OutlinerCtx);
  if (!ctx) throw new Error("useOutliner must be used inside <OutlinerProvider>");
  return ctx;
}

export function OutlinerProvider({ children }: { children: React.ReactNode }) {
  const [version, setVersion] = useState(0);
  const bump = () => setVersion((v) => v + 1);
  const childrenMap = useRef<ChildrenMap>(new Map());
  const loadingSet = useRef<Set<number>>(new Set());
  const errorMap = useRef<Map<number, string>>(new Map());
  const [editingId, setEditing] = useState<number | null>(null);

  const store = useMemo<Store>(() => {
    const getChildren = (parentId: number) => childrenMap.current.get(parentId);

    const ensureLoaded = (parentId: number) => {
      if (childrenMap.current.has(parentId) || loadingSet.current.has(parentId)) return;
      loadingSet.current.add(parentId);
      errorMap.current.delete(parentId);
      listBlockChildren(parentId)
        .then((rows) => {
          childrenMap.current.set(parentId, rows);
        })
        .catch((e) => {
          errorMap.current.set(parentId, String(e));
        })
        .finally(() => {
          loadingSet.current.delete(parentId);
          bump();
        });
    };

    const loadError = (parentId: number) => errorMap.current.get(parentId);

    const replaceBlock = (updated: Node) => {
      const pid = updated.parent_id;
      if (pid === null) return;
      const list = childrenMap.current.get(pid);
      if (!list) return;
      childrenMap.current.set(
        pid,
        list.map((b) => (b.id === updated.id ? updated : b)),
      );
      bump();
    };

    const insertAfter = (parentId: number, afterId: number | null, block: Node) => {
      const list = childrenMap.current.get(parentId) ?? [];
      let next: Node[];
      if (afterId === null) {
        next = [...list, block];
      } else {
        const idx = list.findIndex((b) => b.id === afterId);
        if (idx < 0) next = [...list, block];
        else next = [...list.slice(0, idx + 1), block, ...list.slice(idx + 1)];
      }
      // Keep sorted by position to match server order.
      next.sort((a, b) => (a.position ?? 0) - (b.position ?? 0) || a.id - b.id);
      childrenMap.current.set(parentId, next);
      bump();
    };

    const removeBlock = (parentId: number, id: number) => {
      const list = childrenMap.current.get(parentId);
      if (!list) return;
      childrenMap.current.set(
        parentId,
        list.filter((b) => b.id !== id),
      );
      bump();
    };

    const moveLocal = (block: Node, oldParentId: number) => {
      // Remove from old parent.
      const oldList = childrenMap.current.get(oldParentId);
      if (oldList) {
        childrenMap.current.set(
          oldParentId,
          oldList.filter((b) => b.id !== block.id),
        );
      }
      // Insert into new parent (only if it's loaded; otherwise let lazy load handle it).
      const newPid = block.parent_id;
      if (newPid !== null && childrenMap.current.has(newPid)) {
        const list = childrenMap.current.get(newPid) ?? [];
        const next = [...list.filter((b) => b.id !== block.id), block];
        next.sort((a, b) => (a.position ?? 0) - (b.position ?? 0) || a.id - b.id);
        childrenMap.current.set(newPid, next);
      }
      bump();
    };

    return {
      getChildren,
      ensureLoaded,
      loadError,
      replaceBlock,
      insertAfter,
      removeBlock,
      moveLocal,
      editingId,
      setEditing,
    };
    // version dep so consumers re-render after mutations
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [version, editingId]);

  return <OutlinerCtx.Provider value={store}>{children}</OutlinerCtx.Provider>;
}
