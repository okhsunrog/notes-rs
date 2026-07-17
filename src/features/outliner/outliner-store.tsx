import { createContext, useContext, useEffect, useMemo, useState } from "react";
import type { PageView } from "@/lib/api";

type Store = {
  editingUuid: string | null;
  setEditing: (uuid: string | null) => void;
  view: PageView;
};

const OutlinerCtx = createContext<Store | null>(null);

export function useOutliner() {
  const ctx = useContext(OutlinerCtx);
  if (!ctx) throw new Error("useOutliner must be used inside <OutlinerProvider>");
  return ctx;
}

/** Only ephemeral editor state lives here. Persisted blocks and child lists are
 * owned by Rust/SQLite and cached by TanStack Query. */
export function OutlinerProvider({
  children,
  initialEditingUuid = null,
  initialEditingRequest = 0,
  view,
}: {
  children: React.ReactNode;
  initialEditingUuid?: string | null;
  initialEditingRequest?: number;
  view: PageView;
}) {
  const [editingUuid, setEditing] = useState<string | null>(
    initialEditingRequest > 0 ? initialEditingUuid : null,
  );

  useEffect(() => {
    if (initialEditingRequest > 0 && initialEditingUuid !== null) {
      setEditing(initialEditingUuid);
    }
  }, [initialEditingRequest, initialEditingUuid]);

  useEffect(() => {
    if (view === "reading") setEditing(null);
  }, [view]);

  const store = useMemo(() => ({ editingUuid, setEditing, view }), [editingUuid, view]);

  return <OutlinerCtx.Provider value={store}>{children}</OutlinerCtx.Provider>;
}
