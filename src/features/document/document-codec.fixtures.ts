import type { BlockStyle } from "@/lib/api";
import type { DocumentBlockSnapshot } from "./document-codec";

export const ALL_BLOCK_STYLE_FIXTURES = [
  [{ kind: "paragraph" }, "Paragraph with **Markdown**"],
  [{ kind: "bullet" }, "Bullet"],
  [{ kind: "numbered" }, "Numbered"],
  [{ kind: "task", state: "todo" }, "Todo"],
  [{ kind: "task", state: "doing" }, "Doing"],
  [{ kind: "task", state: "now" }, "Now"],
  [{ kind: "task", state: "later" }, "Later"],
  [{ kind: "task", state: "done" }, "Done"],
  [{ kind: "task", state: "waiting" }, "Waiting"],
  [{ kind: "task", state: "cancelled" }, "Cancelled"],
  [{ kind: "heading_1" }, "Heading one"],
  [{ kind: "heading_2" }, "Heading two"],
  [{ kind: "heading_3" }, "Heading three"],
  [{ kind: "quote" }, "Quoted\nacross lines"],
  [{ kind: "code" }, 'fn main() {\n    println!("hi");\n}'],
  [{ kind: "divider" }, ""],
] as const satisfies readonly (readonly [BlockStyle, string])[];

export function fixtureBlock(
  uuid: string,
  style: BlockStyle,
  markdown: string,
  parentUuid: string | null = null,
): DocumentBlockSnapshot {
  return { uuid, parentUuid, style, markdown };
}
