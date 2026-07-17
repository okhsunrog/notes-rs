import { Check, Copy, ExternalLink, Play } from "lucide-react";
import {
  type AnchorHTMLAttributes,
  type CSSProperties,
  type HTMLAttributes,
  type ImgHTMLAttributes,
  type MouseEvent,
  type ReactNode,
  type TableHTMLAttributes,
  useEffect,
  useMemo,
  useState,
} from "react";
import type { ExtraProps } from "react-markdown";
import {
  classifyMarkdownImageSource,
  type MarkdownImageResolver,
  validateResolvedMarkdownImage,
} from "./image-policy";
import { MermaidDiagram } from "./mermaid-diagram";
import { isMermaidFenceLanguage } from "./mermaid-policy";
import { extractMarkdownCodeLanguage, highlightMarkdownCode } from "./syntax-highlighter";
import { classifyMarkdownUrl } from "./url-policy";
import type { MarkdownOpenHandler, MarkdownRenderContext } from "./types";

interface MarkdownLinkProps extends AnchorHTMLAttributes<HTMLAnchorElement>, ExtraProps {
  context: MarkdownRenderContext;
  onOpenLink?: MarkdownOpenHandler;
}

export function MarkdownLink({
  children,
  context,
  href,
  node: _node,
  onClick: _onClick,
  onOpenLink,
  ...props
}: MarkdownLinkProps) {
  const classified = classifyMarkdownUrl(href ?? "");
  if (classified.kind === "blocked") {
    return (
      <span data-markdown-link="blocked" title="Unsupported link">
        {children}
      </span>
    );
  }

  const open = (event: MouseEvent<HTMLAnchorElement>) => {
    event.stopPropagation();
    if (classified.target.kind === "fragment" && !onOpenLink) return;
    event.preventDefault();
    void onOpenLink?.({
      context,
      disposition: event.shiftKey ? "adjacent" : "current",
      target: classified.target,
    });
  };

  return (
    <a
      {...props}
      href={classified.href}
      data-markdown-link={classified.target.kind}
      onClick={open}
      rel={classified.target.kind === "external" ? "noopener noreferrer" : undefined}
    >
      {children}
    </a>
  );
}

interface MarkdownVideoLinkCardProps {
  context: MarkdownRenderContext;
  href?: string;
  onOpenLink?: MarkdownOpenHandler;
}

/** Privacy-safe Logseq video representation: an inert app-opened URL intent. */
export function MarkdownVideoLinkCard({
  context,
  href = "",
  onOpenLink,
}: MarkdownVideoLinkCardProps) {
  const classified = classifyMarkdownUrl(href);
  if (
    classified.kind !== "allowed" ||
    classified.target.kind !== "external" ||
    classified.target.protocol === "mailto"
  ) {
    return <code data-markdown-video="blocked">{"{{video …}}"}</code>;
  }

  const hostname = new URL(classified.href).hostname.replace(/^www\./i, "");
  return (
    <MarkdownLink
      aria-label={`Open video from ${hostname}`}
      className="markdown-video-card"
      context={context}
      href={classified.href}
      onOpenLink={onOpenLink}
    >
      <span aria-hidden="true" className="markdown-video-card__icon">
        <Play size={16} />
      </span>
      <span className="markdown-video-card__copy">
        <span className="markdown-video-card__title">Video</span>
        <span className="markdown-video-card__host">{hostname}</span>
      </span>
      <ExternalLink aria-hidden="true" className="markdown-video-card__open" size={15} />
    </MarkdownLink>
  );
}

interface MarkdownImageProps extends ImgHTMLAttributes<HTMLImageElement>, ExtraProps {
  context: MarkdownRenderContext;
  inline: boolean;
  resolveImage?: MarkdownImageResolver;
}

export function MarkdownImage({
  alt = "",
  context,
  inline,
  node: _node,
  resolveImage,
  src = "",
  title,
}: MarkdownImageProps) {
  const source = classifyMarkdownImageSource(src);
  if (source.kind === "remote") {
    return <MarkdownImagePlaceholder alt={alt} reason="remote-blocked" />;
  }
  if (source.kind === "blocked") {
    return <MarkdownImagePlaceholder alt={alt} reason="blocked" />;
  }
  if (!resolveImage) {
    return <MarkdownImagePlaceholder alt={alt} reason="unavailable" />;
  }

  let resolved;
  try {
    resolved = resolveImage({
      alt,
      attachmentUuid: source.attachmentUuid,
      context,
      ...(title ? { title } : {}),
    });
  } catch {
    resolved = null;
  }
  if (!resolved) {
    return <MarkdownImagePlaceholder alt={alt} reason="unavailable" />;
  }
  const validated = validateResolvedMarkdownImage(resolved, source.attachmentUuid);
  if (validated.kind === "blocked") {
    return <MarkdownImagePlaceholder alt={alt} reason="blocked" />;
  }

  const image = (
    <img
      alt={alt}
      decoding="async"
      draggable={false}
      height={validated.image.height}
      loading="lazy"
      referrerPolicy="no-referrer"
      src={validated.image.src}
      title={title}
      width={validated.image.width}
    />
  );
  if (inline) return <span className="markdown-image-inline">{image}</span>;
  return (
    <span className="markdown-image" role="group">
      {image}
      {title && <span className="markdown-image-caption">{title}</span>}
    </span>
  );
}

interface MarkdownImagePlaceholderProps {
  alt?: string;
  reason: "blocked" | "remote-blocked" | "unavailable";
}

export function MarkdownImagePlaceholder({ alt, reason }: MarkdownImagePlaceholderProps) {
  const title =
    reason === "remote-blocked"
      ? "Remote images are blocked for privacy"
      : reason === "blocked"
        ? "Unsafe image source"
        : "Attachment is unavailable";
  return (
    <span
      aria-label={alt ? `Image: ${alt}` : "Image"}
      className="markdown-image-placeholder"
      data-markdown-image={reason}
      role="img"
      title={title}
    >
      {alt ? `[Image: ${alt}]` : "[Image]"}
    </span>
  );
}

interface MarkdownTableProps extends TableHTMLAttributes<HTMLTableElement>, ExtraProps {
  children?: ReactNode;
}

export function MarkdownTable({ children, node: _node, ...props }: MarkdownTableProps) {
  return (
    <div
      aria-label="Scrollable table"
      className="markdown-table-scroll"
      data-markdown-table-scroll="true"
      role="region"
      tabIndex={0}
    >
      <table {...props}>{children}</table>
    </div>
  );
}

interface MarkdownCodeProps extends HTMLAttributes<HTMLElement>, ExtraProps {
  children?: ReactNode;
  inlineRenderer: boolean;
}

export function MarkdownCode({
  children,
  className,
  inlineRenderer,
  node: _node,
}: MarkdownCodeProps) {
  const rawCode = typeof children === "string" ? children : "";
  const requestedLanguage = extractMarkdownCodeLanguage(className);
  const isBlock = !inlineRenderer && (requestedLanguage !== null || rawCode.includes("\n"));
  const code = isBlock && rawCode.endsWith("\n") ? rawCode.slice(0, -1) : rawCode;
  const highlighted = useMemo(
    () => (isBlock ? highlightMarkdownCode(code, requestedLanguage) : null),
    [code, isBlock, requestedLanguage],
  );
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1_500);
    return () => window.clearTimeout(timer);
  }, [copied]);

  if (isBlock && isMermaidFenceLanguage(requestedLanguage)) {
    return <MermaidDiagram source={code} />;
  }

  if (!highlighted) return <code className={className}>{children}</code>;

  const label = highlighted.requestedLanguage ?? "Plain text";
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div
      className="markdown-code-block"
      data-code-highlighted={String(highlighted.highlighted)}
      data-code-language={highlighted.language}
    >
      <div className="markdown-code-toolbar">
        <span>{label}</span>
        <button aria-label={copied ? "Code copied" : "Copy code"} onClick={copy} type="button">
          {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
        </button>
      </div>
      <pre tabIndex={0}>
        <code>
          {highlighted.lines.map((line, lineIndex) => (
            <span className="markdown-code-line" key={`line-${lineIndex}`}>
              {line.map((token, tokenIndex) => {
                const style = {
                  "--shiki-dark": token.darkColor ?? token.lightColor,
                  "--shiki-light": token.lightColor,
                } as CSSProperties;
                return (
                  <span
                    className="markdown-code-token"
                    key={`token-${lineIndex}-${tokenIndex}`}
                    style={style}
                  >
                    {token.content}
                  </span>
                );
              })}
              {lineIndex < highlighted.lines.length - 1 ? "\n" : null}
            </span>
          ))}
        </code>
      </pre>
    </div>
  );
}

export function MarkdownPre({ children }: HTMLAttributes<HTMLPreElement>) {
  return <>{children}</>;
}
