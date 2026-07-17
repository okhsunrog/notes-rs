import { useMemo } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { MarkdownImagePlaceholder, MarkdownLink, MarkdownTable } from "./markdown-components";
import { remarkNotesLinks } from "./remark-notes-links";
import type { MarkdownOpenHandler, MarkdownRenderContext } from "./types";
import { safeMarkdownUrlTransform } from "./url-policy";

const ALLOWED_ELEMENTS = [
  "a",
  "blockquote",
  "br",
  "code",
  "del",
  "em",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "hr",
  "img",
  "input",
  "li",
  "ol",
  "p",
  "pre",
  "strong",
  "table",
  "tbody",
  "td",
  "th",
  "thead",
  "tr",
  "ul",
] as const;

const INLINE_ELEMENTS = ["a", "br", "code", "del", "em", "img", "strong"] as const;

export type MarkdownRenderMode = "flow" | "inline";

export interface MarkdownRendererProps {
  className?: string;
  context: MarkdownRenderContext;
  markdown: string;
  mode?: MarkdownRenderMode;
  onOpenLink?: MarkdownOpenHandler;
}

/** Secure semantic renderer shared by notes, previews, and assistant output. */
export function MarkdownRenderer({
  className,
  context,
  markdown,
  mode = "flow",
  onOpenLink,
}: MarkdownRendererProps) {
  const components = useMemo<Components>(
    () => ({
      a: (props) => <MarkdownLink {...props} context={context} onOpenLink={onOpenLink} />,
      img: MarkdownImagePlaceholder,
      table: MarkdownTable,
    }),
    [context, onOpenLink],
  );

  const content = (
    <ReactMarkdown
      allowedElements={mode === "inline" ? INLINE_ELEMENTS : ALLOWED_ELEMENTS}
      components={components}
      remarkPlugins={[remarkGfm, remarkNotesLinks]}
      skipHtml
      unwrapDisallowed={mode === "inline"}
      urlTransform={safeMarkdownUrlTransform}
    >
      {markdown}
    </ReactMarkdown>
  );

  return mode === "inline" ? (
    <span className={className} data-markdown-context={context.kind}>
      {content}
    </span>
  ) : (
    <div className={className} data-markdown-context={context.kind}>
      {content}
    </div>
  );
}
