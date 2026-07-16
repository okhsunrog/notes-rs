import { createContext, useContext, useEffect, useMemo, useState } from "react";

type Store = {
  editingId: number | null;
  setEditing: (id: number | null) => void;
};

const OutlinerCtx = createContext<Store | null>(null);

export function useOutliner() {
  const ctx = useContext(OutlinerCtx);
  if (!ctx) throw new Error("useOutliner must be used inside <OutlinerProvider>");
  return ctx;
}

/** Only ephemeral editor state lives here. Persisted nodes and child lists are
 * owned by Rust/SQLite and cached by TanStack Query. */
export function OutlinerProvider({
  children,
  initialEditingId = null,
}: {
  children: React.ReactNode;
  initialEditingId?: number | null;
}) {
  const [editingId, setEditing] = useState<number | null>(initialEditingId);

  useEffect(() => {
    setEditing(initialEditingId);
  }, [initialEditingId]);

  const store = useMemo(() => ({ editingId, setEditing }), [editingId]);

  return <OutlinerCtx.Provider value={store}>{children}</OutlinerCtx.Provider>;
}
