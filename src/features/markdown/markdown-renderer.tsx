import "katex/dist/katex.min.css";

import { useMemo } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeKatex from "rehype-katex";
import rehypeSanitize, {
  defaultSchema,
  type Options as MarkdownSanitizeSchema,
} from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import type { MarkdownImageResolver } from "./image-policy";
import {
  MarkdownCode,
  MarkdownImage,
  MarkdownLink,
  MarkdownPre,
  MarkdownTable,
} from "./markdown-components";
import { remarkMathLimits } from "./remark-math-limits";
import { remarkNotesLinks } from "./remark-notes-links";
import type { MarkdownOpenHandler, MarkdownRenderContext } from "./types";
import { safeMarkdownUrlTransform } from "./url-policy";

const INLINE_BLOCK_ELEMENTS = [
  "blockquote",
  "div",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "hr",
  "li",
  "ol",
  "p",
  "pre",
  "section",
  "table",
  "tbody",
  "td",
  "tfoot",
  "th",
  "thead",
  "tr",
  "ul",
] as const;

const MARKDOWN_SANITIZE_SCHEMA: MarkdownSanitizeSchema = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    code: [...(defaultSchema.attributes?.code ?? []), ["className", "math-inline", "math-display"]],
  },
  protocols: {
    ...defaultSchema.protocols,
    href: [...(defaultSchema.protocols?.href ?? []), "notes-page", "notes-block"],
    src: [...(defaultSchema.protocols?.src ?? []), "notes-attachment"],
  },
};

export type MarkdownRenderMode = "flow" | "inline";

export interface MarkdownRendererProps {
  className?: string;
  context: MarkdownRenderContext;
  markdown: string;
  mode?: MarkdownRenderMode;
  onOpenLink?: MarkdownOpenHandler;
  resolveImage?: MarkdownImageResolver;
}

/** Secure semantic renderer shared by notes, previews, and assistant output. */
export function MarkdownRenderer({
  className,
  context,
  markdown,
  mode = "flow",
  onOpenLink,
  resolveImage,
}: MarkdownRendererProps) {
  const components = useMemo<Components>(
    () => ({
      a: (props) => <MarkdownLink {...props} context={context} onOpenLink={onOpenLink} />,
      code: (props) => <MarkdownCode {...props} inlineRenderer={mode === "inline"} />,
      img: (props) => (
        <MarkdownImage
          {...props}
          context={context}
          inline={mode === "inline"}
          resolveImage={resolveImage}
        />
      ),
      pre: MarkdownPre,
      table: MarkdownTable,
    }),
    [context, mode, onOpenLink, resolveImage],
  );
  const rootClassName = ["markdown-renderer", className].filter(Boolean).join(" ");

  const content = (
    <ReactMarkdown
      components={components}
      disallowedElements={mode === "inline" ? INLINE_BLOCK_ELEMENTS : undefined}
      rehypePlugins={[
        [rehypeSanitize, MARKDOWN_SANITIZE_SCHEMA],
        [
          rehypeKatex,
          {
            maxExpand: 1_000,
            maxSize: 50,
            output: "htmlAndMathml",
            strict: "ignore",
            trust: false,
          },
        ],
      ]}
      remarkPlugins={[remarkGfm, remarkMath, remarkMathLimits, remarkNotesLinks]}
      skipHtml
      unwrapDisallowed={mode === "inline"}
      urlTransform={safeMarkdownUrlTransform}
    >
      {markdown}
    </ReactMarkdown>
  );

  return mode === "inline" ? (
    <span className={rootClassName} data-markdown-context={context.kind}>
      {content}
    </span>
  ) : (
    <div className={rootClassName} data-markdown-context={context.kind}>
      {content}
    </div>
  );
}
