import { AlertTriangle, LoaderCircle } from "lucide-react";
import { useTheme } from "next-themes";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import {
  MermaidRequestController,
  mermaidCacheKey,
  stableMermaidId,
  validateMermaidSource,
  type MermaidTheme,
} from "./mermaid-policy";
import { cachedMermaidResult, renderMermaid, type MermaidRenderResult } from "./mermaid-runtime";

interface MermaidDiagramProps {
  source: string;
}

type DiagramState = MermaidRenderResult | { kind: "loading" } | { kind: "oversize" };

export function MermaidDiagram({ source }: MermaidDiagramProps) {
  const { resolvedTheme } = useTheme();
  const theme: MermaidTheme = resolvedTheme === "dark" ? "dark" : "light";
  const reactId = useId();
  const key = mermaidCacheKey(source, theme);
  const diagramId = useMemo(() => stableMermaidId(key, reactId), [key, reactId]);
  const controller = useRef(new MermaidRequestController());
  const validation = validateMermaidSource(source);
  const [state, setState] = useState<DiagramState>(() => {
    if (validation.kind === "oversize") return { kind: "oversize" };
    return cachedMermaidResult(source, theme) ?? { kind: "loading" };
  });

  useEffect(() => {
    if (validation.kind === "oversize") {
      controller.current.cancel();
      setState({ kind: "oversize" });
      return;
    }
    const cached = cachedMermaidResult(source, theme);
    if (cached) {
      controller.current.cancel();
      setState(cached);
      return;
    }

    const request = controller.current.start();
    setState({ kind: "loading" });
    void renderMermaid(diagramId, source, theme, request.signal).then(
      (result) => {
        if (controller.current.isCurrent(request.id)) setState(result);
      },
      () => undefined,
    );
    return () => controller.current.cancel(request.id);
  }, [diagramId, source, theme, validation.kind]);

  if (state.kind === "success") {
    return (
      <div
        aria-label="Mermaid diagram"
        className="markdown-mermaid"
        data-mermaid-state="ready"
        role="img"
      >
        <img alt="Rendered Mermaid diagram" draggable={false} src={svgDataUrl(state.svg)} />
      </div>
    );
  }

  const loading = state.kind === "loading";
  const message = loading
    ? "Rendering Mermaid diagram…"
    : state.kind === "oversize"
      ? "Diagram is too large to render safely."
      : state.message;
  return (
    <div
      aria-live="polite"
      className="markdown-mermaid markdown-mermaid--fallback"
      data-mermaid-state={state.kind}
      role={loading ? "status" : "alert"}
    >
      <div className="markdown-mermaid__status">
        {loading ? (
          <LoaderCircle aria-hidden="true" className="markdown-mermaid__spinner" />
        ) : (
          <AlertTriangle aria-hidden="true" />
        )}
        <span>{message}</span>
      </div>
      <pre tabIndex={0}>
        <code>{source}</code>
      </pre>
    </div>
  );
}

function svgDataUrl(svg: string): string {
  const bytes = new TextEncoder().encode(svg);
  let binary = "";
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return `data:image/svg+xml;base64,${window.btoa(binary)}`;
}
