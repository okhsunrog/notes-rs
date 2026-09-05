import type { InkDraft, InkDraftPatch } from "@/lib/bindings";

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
