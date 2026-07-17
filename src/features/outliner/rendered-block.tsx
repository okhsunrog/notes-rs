import { MarkdownRenderer, type MarkdownOpenHandler } from "@/features/markdown";
import type { Block, PageLayout } from "@/lib/api";

export interface RenderedBlockProps {
  block: Block;
  layout: PageLayout;
  onOpenLink: MarkdownOpenHandler;
  ordinal: number;
  readOnly: boolean;
}

/** Renders the persisted block style around the shared notes Markdown dialect. */
export function RenderedBlock({
  block,
  layout,
  onOpenLink,
  ordinal,
  readOnly,
}: RenderedBlockProps) {
  if (block.style === "divider") return <hr className="my-4 border-border/70" />;
  if (!block.markdown) {
    return <span className="text-sm text-muted-foreground/45">Start writing…</span>;
  }
  if (block.style === "code") {
    return (
      <pre className="overflow-x-auto rounded-xl border border-border/60 bg-muted/55 p-3 text-xs leading-relaxed">
        <code>{block.markdown}</code>
      </pre>
    );
  }

  const markdown = (
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
    />
  );

  if (block.style === "heading_1") {
    return <h2 className="mt-5 mb-2 text-2xl font-semibold tracking-tight">{markdown}</h2>;
  }
  if (block.style === "heading_2") {
    return <h3 className="mt-4 mb-1.5 text-xl font-semibold tracking-tight">{markdown}</h3>;
  }
  if (block.style === "heading_3") {
    return <h4 className="mt-3 mb-1 text-base font-semibold">{markdown}</h4>;
  }
  if (block.style === "quote") {
    return (
      <blockquote className="border-l-2 border-primary/35 pl-4 text-sm leading-relaxed text-muted-foreground italic">
        {markdown}
      </blockquote>
    );
  }
  if (layout === "outline" && (block.style === "bullet" || block.style === "numbered")) {
    return <p className="whitespace-pre-wrap break-words text-sm leading-relaxed">{markdown}</p>;
  }
  if (block.style === "bullet" || block.style === "numbered" || block.style === "task") {
    return (
      <div className="flex gap-2 text-sm leading-relaxed">
        {block.style === "task" ? (
          <input type="checkbox" disabled className="mt-1 size-3.5 accent-primary" />
        ) : (
          <span className="w-4 shrink-0 text-right text-muted-foreground">
            {block.style === "numbered" ? `${ordinal}.` : "•"}
          </span>
        )}
        <span className="min-w-0 whitespace-pre-wrap break-words">{markdown}</span>
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
    />
  );
}
