import type { Block } from "@/lib/api";

export type JournalOutlineRow = {
  block: Block;
  depth: number;
};

export function journalOutlineRows(blocks: Block[]): JournalOutlineRow[] {
  const depthByUuid = new Map<string, number>();
  const rows: JournalOutlineRow[] = [];

  for (const block of blocks) {
    const depth = block.parentUuid ? (depthByUuid.get(block.parentUuid) ?? -1) + 1 : 0;
    depthByUuid.set(block.uuid, depth);
    if (block.style.kind === "divider" || block.markdown.trim()) rows.push({ block, depth });
  }

  return rows;
}

export function journalPreviewLimitForHeight(height: number) {
  if (height < 720) return 3;
  if (height < 900) return 4;
  return 6;
}
