import type { InkDraft, InkDraftPatch, InkHistorySnapshot, InkHistoryUpdate } from "@/lib/bindings";

/** Reuse unchanged objects so subsequent saves do not resend their geometry. */
export function applyHistoryUpdate(
  current: InkDraft | null,
  revision: string | null,
  update: InkHistoryUpdate,
): InkHistorySnapshot {
  if (update.kind === "snapshot") return update.history;
  if (!current || revision !== update.baseRevision)
    throw new Error("Handwriting history changed. Reopen the page.");
  const strokes = new Map(current.strokes.map((stroke) => [stroke.id, stroke]));
  for (const stroke of update.patch.upserts) strokes.set(stroke.id, stroke);
  const ordered = update.patch.order.map((id) => {
    const stroke = strokes.get(id);
    if (!stroke) throw new Error("Incomplete handwriting history. Reopen the page.");
    return stroke;
  });
  return {
    snapshot: {
      draft: { ...current, strokes: ordered, background: update.patch.background },
      revision: update.revision,
    },
    canUndo: update.canUndo,
    canRedo: update.canRedo,
  };
}

/** Advance the comparison snapshot only after storage acknowledges the write. */
export function incrementalDraftSaver(
  initial: InkDraft,
  save: (patch: InkDraftPatch, revision: string | null) => Promise<string>,
) {
  let acknowledged = initial;
  return async (next: InkDraft, revision: string | null) => {
    const previous = new Set(acknowledged.strokes);
    const result = await save(
      {
        order: next.strokes.map((stroke) => stroke.id),
        upserts: next.strokes.filter((stroke) => !previous.has(stroke)),
        background: next.background ?? "plain",
      },
      revision,
    );
    acknowledged = next;
    return result;
  };
}
