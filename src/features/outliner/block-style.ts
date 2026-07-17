import type { Block, BlockStyle, TaskState } from "@/lib/api";

export type BlockStyleKind = BlockStyle["kind"];

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
  value: BlockStyleKind;
  label: string;
  icon: BlockStyleIcon;
};

export type TaskStateOption = {
  value: TaskState;
  label: string;
};

/** The single, compile-time exhaustive UI catalogue for the generated RPC type. */
const BLOCK_STYLE_OPTIONS_BY_KIND = {
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
} as const satisfies Record<BlockStyleKind, BlockStyleOption>;

const TASK_STATE_OPTIONS_BY_VALUE = {
  todo: { value: "todo", label: "Todo" },
  doing: { value: "doing", label: "Doing" },
  now: { value: "now", label: "Now" },
  later: { value: "later", label: "Later" },
  done: { value: "done", label: "Done" },
  waiting: { value: "waiting", label: "Waiting" },
  cancelled: { value: "cancelled", label: "Cancelled" },
} as const satisfies Record<TaskState, TaskStateOption>;

export const BLOCK_STYLE_OPTIONS: readonly BlockStyleOption[] = Object.values(
  BLOCK_STYLE_OPTIONS_BY_KIND,
);

export const TASK_STATE_OPTIONS: readonly TaskStateOption[] = Object.values(
  TASK_STATE_OPTIONS_BY_VALUE,
);

export function isBlockStyleKind(value: string): value is BlockStyleKind {
  return Object.prototype.hasOwnProperty.call(BLOCK_STYLE_OPTIONS_BY_KIND, value);
}

export function isTaskState(value: string): value is TaskState {
  return Object.prototype.hasOwnProperty.call(TASK_STATE_OPTIONS_BY_VALUE, value);
}

export function blockStyleForKind(kind: BlockStyleKind, current?: BlockStyle): BlockStyle {
  if (kind === "task") {
    return current?.kind === "task" ? current : { kind: "task", state: "todo" };
  }
  return { kind };
}

export function blockStylesEqual(left: BlockStyle, right: BlockStyle): boolean {
  return (
    left.kind === right.kind &&
    (left.kind !== "task" || (right.kind === "task" && left.state === right.state))
  );
}

export function getBlockStyleOption(style: BlockStyle): BlockStyleOption {
  return BLOCK_STYLE_OPTIONS_BY_KIND[style.kind];
}

export function getTaskStateOption(state: TaskState): TaskStateOption {
  return TASK_STATE_OPTIONS_BY_VALUE[state];
}

/** Checkbox behavior mirrors `TaskState::toggled` in notes-core. */
export function toggledTaskState(state: TaskState): TaskState {
  return state === "done" || state === "cancelled" ? "todo" : "done";
}

/** Replace a persisted block snapshot without disturbing sibling order or identity. */
export function replaceCachedBlock(rows: Block[], updated: Block): Block[] {
  return rows.map((row) => (row.uuid === updated.uuid ? updated : row));
}
