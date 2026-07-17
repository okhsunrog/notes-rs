import type {
  AnchorHTMLAttributes,
  ImgHTMLAttributes,
  MouseEvent,
  ReactNode,
  TableHTMLAttributes,
} from "react";
import type { ExtraProps } from "react-markdown";
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

interface MarkdownImagePlaceholderProps extends ImgHTMLAttributes<HTMLImageElement>, ExtraProps {}

/** Images remain inert until the typed attachment resolver is implemented. */
export function MarkdownImagePlaceholder({ alt, node: _node }: MarkdownImagePlaceholderProps) {
  return (
    <span aria-label={alt ? `Image: ${alt}` : "Image"} data-markdown-image="unavailable" role="img">
      {alt ? `[Image: ${alt}]` : "[Image]"}
    </span>
  );
}

interface MarkdownTableProps extends TableHTMLAttributes<HTMLTableElement>, ExtraProps {
  children?: ReactNode;
}

export function MarkdownTable({ children, node: _node, ...props }: MarkdownTableProps) {
  return (
    <div data-markdown-table-scroll="true">
      <table {...props}>{children}</table>
    </div>
  );
}
