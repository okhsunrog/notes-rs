import { createContext, useContext, useEffect, useMemo, useState } from "react";
import type { MarkdownOpenHandler } from "@/features/markdown";
import type { PageLayout } from "@/lib/api";

type Store = {
  editingUuid: string | null;
  setEditing: (uuid: string | null) => void;
  layout: PageLayout;
  readOnly: boolean;
  onOpenMarkdownLink: MarkdownOpenHandler;
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
  layout,
  onOpenMarkdownLink,
  readOnly,
}: {
  children: React.ReactNode;
  initialEditingUuid?: string | null;
  initialEditingRequest?: number;
  layout: PageLayout;
  onOpenMarkdownLink: MarkdownOpenHandler;
  readOnly: boolean;
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
    if (readOnly) setEditing(null);
  }, [readOnly]);

  const store = useMemo(
    () => ({ editingUuid, setEditing, layout, onOpenMarkdownLink, readOnly }),
    [editingUuid, layout, onOpenMarkdownLink, readOnly],
  );

  return <OutlinerCtx.Provider value={store}>{children}</OutlinerCtx.Provider>;
}
