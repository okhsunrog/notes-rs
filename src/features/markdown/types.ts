export type MarkdownPresentation = "reading" | "live_preview";

export type MarkdownRenderContext =
  | {
      kind: "note";
      presentation: MarkdownPresentation;
      pageUuid: string;
      blockUuid?: string;
    }
  | {
      kind: "preview";
      pageUuid?: string;
      blockUuid?: string;
    }
  | {
      kind: "assistant";
    };

export type MarkdownOpenDisposition = "current" | "adjacent";

export type MarkdownLinkTarget =
  | {
      kind: "page";
      title: string;
    }
  | {
      kind: "block";
      uuid: string;
    }
  | {
      kind: "external";
      href: string;
      protocol: "http" | "https" | "mailto";
    }
  | {
      kind: "fragment";
      fragment: string;
    };

export interface MarkdownOpenRequest {
  context: MarkdownRenderContext;
  disposition: MarkdownOpenDisposition;
  target: MarkdownLinkTarget;
}

export type MarkdownOpenHandler = (request: MarkdownOpenRequest) => void | Promise<void>;
