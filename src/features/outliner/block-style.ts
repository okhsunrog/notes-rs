import type { Block, BlockStyle } from "@/lib/api";

export type BlockStyleIcon =
  | "paragraph"
  | "bullet"
  | "numbered"
  | "task"
  | "heading-1"
  | "heading-2"
  | "heading-3"
  | "quote"
  | "code"
  | "divider";

export type BlockStyleOption = {
  value: BlockStyle;
  label: string;
  icon: BlockStyleIcon;
};

/** The single, compile-time exhaustive UI catalogue for the generated RPC type. */
const BLOCK_STYLE_OPTIONS_BY_VALUE = {
  paragraph: { value: "paragraph", label: "Text", icon: "paragraph" },
  bullet: { value: "bullet", label: "Bulleted list", icon: "bullet" },
  numbered: { value: "numbered", label: "Numbered list", icon: "numbered" },
  task: { value: "task", label: "Task", icon: "task" },
  heading_1: { value: "heading_1", label: "Heading 1", icon: "heading-1" },
  heading_2: { value: "heading_2", label: "Heading 2", icon: "heading-2" },
  heading_3: { value: "heading_3", label: "Heading 3", icon: "heading-3" },
  quote: { value: "quote", label: "Quote", icon: "quote" },
  code: { value: "code", label: "Code block", icon: "code" },
  divider: { value: "divider", label: "Divider", icon: "divider" },
} as const satisfies Record<BlockStyle, BlockStyleOption>;

export const BLOCK_STYLE_OPTIONS: readonly BlockStyleOption[] = Object.values(
  BLOCK_STYLE_OPTIONS_BY_VALUE,
);

export function isBlockStyle(value: string): value is BlockStyle {
  return Object.prototype.hasOwnProperty.call(BLOCK_STYLE_OPTIONS_BY_VALUE, value);
}

export function getBlockStyleOption(style: BlockStyle): BlockStyleOption {
  return BLOCK_STYLE_OPTIONS_BY_VALUE[style];
}

/** Replace a persisted block snapshot without disturbing sibling order or identity. */
export function replaceCachedBlock(rows: Block[], updated: Block): Block[] {
  return rows.map((row) => (row.uuid === updated.uuid ? updated : row));
}
