import type { Block } from "@/lib/api";

/** Returns the previous sibling (or null if `currentUuid` is first / absent). */
export function prevSibling(siblings: Block[], currentUuid: string): Block | null {
  const idx = siblings.findIndex((block) => block.uuid === currentUuid);
  if (idx <= 0) return null;
  return siblings[idx - 1];
}

/** Returns the next sibling, or null. */
export function nextSibling(siblings: Block[], currentUuid: string): Block | null {
  const idx = siblings.findIndex((block) => block.uuid === currentUuid);
  if (idx < 0 || idx === siblings.length - 1) return null;
  return siblings[idx + 1];
}
