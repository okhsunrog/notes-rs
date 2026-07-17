import { Check, Circle, CircleSlash, Loader2 } from "lucide-react";
import {
  MarkdownRenderer,
  type MarkdownOpenHandler,
  useAttachmentImageResolver,
} from "@/features/markdown";
import type { Block, PageLayout, TaskState } from "@/lib/api";
import { getTaskStateOption, toggledTaskState } from "./block-style";

export interface RenderedBlockProps {
  block: Block;
  layout: PageLayout;
  onOpenLink: MarkdownOpenHandler;
  ordinal: number;
  readOnly: boolean;
  taskBusy: boolean;
  onTaskStateChange: (state: TaskState) => void | Promise<void>;
}

/** Renders the persisted block style around the shared notes Markdown dialect. */
export function RenderedBlock({
  block,
  layout,
  onOpenLink,
  ordinal,
  readOnly,
  taskBusy,
  onTaskStateChange,
}: RenderedBlockProps) {
  const resolveImage = useAttachmentImageResolver(block.markdown);
  if (block.style.kind === "divider") return <hr className="my-4 border-border/70" />;
  if (!block.markdown && block.style.kind !== "task") {
    return <span className="text-sm text-muted-foreground/45">Start writing…</span>;
  }
  if (block.style.kind === "code") {
    return (
      <pre className="overflow-x-auto rounded-xl border border-border/60 bg-muted/55 p-3 text-xs leading-relaxed">
        <code>{block.markdown}</code>
      </pre>
    );
  }

  const inlineMarkdown = (
    <MarkdownRenderer
      context={{
        kind: "note",
        presentation: readOnly ? "reading" : "live_preview",
        pageUuid: block.pageUuid,
        blockUuid: block.uuid,
      }}
      markdown={block.markdown}
      mode="inline"
      onOpenLink={onOpenLink}
      resolveImage={resolveImage}
    />
  );
  const compactMarkdown = (
    <MarkdownRenderer
      context={{
        kind: "note",
        presentation: readOnly ? "reading" : "live_preview",
        pageUuid: block.pageUuid,
        blockUuid: block.uuid,
      }}
      markdown={block.markdown}
      mode="compact_flow"
      onOpenLink={onOpenLink}
      resolveImage={resolveImage}
    />
  );
  const listContent = block.markdown ? (
    compactMarkdown
  ) : (
    <span className="text-muted-foreground/45">Start writing…</span>
  );

  if (block.style.kind === "heading_1") {
    return <h2 className="mt-5 mb-2 text-2xl font-semibold tracking-tight">{inlineMarkdown}</h2>;
  }
  if (block.style.kind === "heading_2") {
    return <h3 className="mt-4 mb-1.5 text-xl font-semibold tracking-tight">{inlineMarkdown}</h3>;
  }
  if (block.style.kind === "heading_3") {
    return <h4 className="mt-3 mb-1 text-base font-semibold">{inlineMarkdown}</h4>;
  }
  if (block.style.kind === "quote") {
    return (
      <blockquote className="border-l-2 border-primary/35 pl-4 text-sm leading-relaxed text-muted-foreground italic">
        {inlineMarkdown}
      </blockquote>
    );
  }
  if (layout === "outline" && (block.style.kind === "bullet" || block.style.kind === "numbered")) {
    return <div className="min-w-0 break-words text-sm leading-relaxed">{compactMarkdown}</div>;
  }
  if (
    block.style.kind === "bullet" ||
    block.style.kind === "numbered" ||
    block.style.kind === "task"
  ) {
    return (
      <div className="flex gap-2 text-sm leading-relaxed">
        {block.style.kind === "task" ? (
          <TaskCheckbox
            state={block.style.state}
            busy={taskBusy}
            readOnly={readOnly}
            onChange={onTaskStateChange}
          />
        ) : (
          <span className="w-4 shrink-0 text-right text-muted-foreground">
            {block.style.kind === "numbered" ? `${ordinal}.` : "•"}
          </span>
        )}
        <div className="min-w-0 flex-1 break-words">{listContent}</div>
      </div>
    );
  }

  return (
    <MarkdownRenderer
      className="whitespace-pre-wrap break-words text-sm leading-relaxed"
      context={{
        kind: "note",
        presentation: readOnly ? "reading" : "live_preview",
        pageUuid: block.pageUuid,
        blockUuid: block.uuid,
      }}
      markdown={block.markdown}
      onOpenLink={onOpenLink}
      resolveImage={resolveImage}
    />
  );
}

function TaskCheckbox({
  state,
  busy,
  readOnly,
  onChange,
}: {
  state: TaskState;
  busy: boolean;
  readOnly: boolean;
  onChange: (state: TaskState) => void | Promise<void>;
}) {
  const terminal = state === "done" || state === "cancelled";
  const next = toggledTaskState(state);
  const label = getTaskStateOption(state).label;
  const nextLabel = getTaskStateOption(next).label;
  const Icon = busy
    ? Loader2
    : state === "done"
      ? Check
      : state === "cancelled"
        ? CircleSlash
        : Circle;

  if (readOnly) {
    return (
      <span
        role="img"
        aria-label={`${label} task`}
        title={`${label} task`}
        className="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-md border border-primary/20 text-primary"
      >
        <Icon className={`size-3.5 ${terminal ? "stroke-[2.5]" : ""}`} />
      </span>
    );
  }

  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={state === "cancelled" ? "mixed" : state === "done"}
      aria-label={`${label} task; change to ${nextLabel}`}
      title={`${label} · change to ${nextLabel}`}
      disabled={busy}
      onClick={(event) => {
        event.stopPropagation();
        void onChange(next);
      }}
      className="flex size-6 shrink-0 items-center justify-center rounded-md border border-primary/25 text-primary transition hover:bg-primary/10 disabled:opacity-60"
    >
      <Icon className={`size-3.5 ${busy ? "animate-spin" : terminal ? "stroke-[2.5]" : ""}`} />
    </button>
  );
}
