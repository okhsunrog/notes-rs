import type { DraftWriter } from "./draft-writer";

/** The boundary completes only after durable writes and compaction. Navigation
 * may leave after the writes; any future publication must await the whole call. */
export async function completeDraft(
  writer: DraftWriter,
  compact: () => Promise<unknown>,
  afterSave?: () => void,
): Promise<boolean> {
  if (!(await writer.flush())) return false;
  afterSave?.();
  await compact();
  return true;
}
