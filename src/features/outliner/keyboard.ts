import type { Node } from "@/lib/api";

/** Position math: midpoint between current block and its next sibling, or
 * `current + 1` if current is the last. Uses fractional indexing so we don't
 * need to renumber siblings. */
export function positionAfter(siblings: Node[], currentId: number): number {
  const idx = siblings.findIndex((b) => b.id === currentId);
  if (idx < 0) {
    // Fallback: append.
    const last = siblings[siblings.length - 1]?.position ?? 0;
    return last + 1.0;
  }
  const cur = siblings[idx].position ?? Number(idx);
  const next = siblings[idx + 1]?.position;
  if (next === undefined || next === null) return cur + 1.0;
  return (cur + next) / 2.0;
}

/** Returns the prev sibling (or null if `currentId` is the first / not found). */
export function prevSibling(siblings: Node[], currentId: number): Node | null {
  const idx = siblings.findIndex((b) => b.id === currentId);
  if (idx <= 0) return null;
  return siblings[idx - 1];
}

/** Returns the next sibling, or null. */
export function nextSibling(siblings: Node[], currentId: number): Node | null {
  const idx = siblings.findIndex((b) => b.id === currentId);
  if (idx < 0 || idx === siblings.length - 1) return null;
  return siblings[idx + 1];
}
