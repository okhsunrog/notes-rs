import "./markdown-video-card.css";

import { useEffect, useMemo, useSyncExternalStore } from "react";
import ReactMarkdown, {
  type Components,
  type Options as ReactMarkdownOptions,
} from "react-markdown";
import rehypeSanitize, {
  defaultSchema,
  type Options as MarkdownSanitizeSchema,
} from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import type { MarkdownImageResolver } from "./image-policy";
import {
  cachedMathRuntime,
  loadMathRuntime,
  markdownMayContainMath,
  subscribeMathRuntime,
} from "./math-runtime";
import {
  MarkdownCode,
  MarkdownImage,
  MarkdownLink,
  MarkdownPre,
  MarkdownTable,
  MarkdownVideoLinkCard,
} from "./markdown-components";
import { remarkLogseqVideo } from "./remark-logseq-video";
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
  tagNames: [...(defaultSchema.tagNames ?? []), "notes-video"],
  attributes: {
    ...defaultSchema.attributes,
    code: [...(defaultSchema.attributes?.code ?? []), ["className", "math-inline", "math-display"]],
    "notes-video": ["href"],
  },
  protocols: {
    ...defaultSchema.protocols,
    href: [...(defaultSchema.protocols?.href ?? []), "notes-page", "notes-block"],
    src: [...(defaultSchema.protocols?.src ?? []), "notes-attachment"],
  },
};

export type MarkdownRenderMode = "flow" | "compact_flow" | "inline";

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
  const mathRuntime = useSyncExternalStore(
    subscribeMathRuntime,
    cachedMathRuntime,
    cachedMathRuntime,
  );
  const needsMath = markdownMayContainMath(markdown);
  useEffect(() => {
    if (needsMath && !mathRuntime) void loadMathRuntime();
  }, [mathRuntime, needsMath]);
  const components = useMemo<Components>(
    () =>
      ({
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
        "notes-video": (props: { href?: unknown }) => (
          <MarkdownVideoLinkCard
            context={context}
            href={typeof props.href === "string" ? props.href : undefined}
            onOpenLink={onOpenLink}
          />
        ),
        pre: MarkdownPre,
        table: MarkdownTable,
      }) as Components,
    [context, mode, onOpenLink, resolveImage],
  );
  const inline = mode === "inline";
  const rootClassName = [
    "markdown-renderer",
    context.kind === "note" && !inline ? "note-prose" : undefined,
    mode === "compact_flow" ? "note-prose--compact-flow" : undefined,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  const rehypePlugins: NonNullable<ReactMarkdownOptions["rehypePlugins"]> = [
    [rehypeSanitize, MARKDOWN_SANITIZE_SCHEMA],
  ];
  const remarkPlugins: NonNullable<ReactMarkdownOptions["remarkPlugins"]> = [remarkGfm];
  if (mathRuntime) {
    rehypePlugins.push([
      mathRuntime.rehypeKatex,
      {
        maxExpand: 1_000,
        maxSize: 50,
        output: "htmlAndMathml",
        strict: "ignore",
        trust: false,
      },
    ]);
    remarkPlugins.push(mathRuntime.remarkMath, remarkMathLimits);
  }
  remarkPlugins.push(remarkLogseqVideo, remarkNotesLinks);

  const content = (
    <ReactMarkdown
      components={components}
      disallowedElements={inline ? INLINE_BLOCK_ELEMENTS : undefined}
      rehypePlugins={rehypePlugins}
      remarkPlugins={remarkPlugins}
      skipHtml
      unwrapDisallowed={inline}
      urlTransform={safeMarkdownUrlTransform}
    >
      {markdown}
    </ReactMarkdown>
  );

  return inline ? (
    <span className={rootClassName} data-markdown-context={context.kind} data-markdown-mode={mode}>
      {content}
    </span>
  ) : (
    <div className={rootClassName} data-markdown-context={context.kind} data-markdown-mode={mode}>
      {content}
    </div>
  );
}
