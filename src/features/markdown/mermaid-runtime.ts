import {
  BoundedLruCache,
  MERMAID_CACHE_ENTRIES,
  MERMAID_RENDER_TIMEOUT_MS,
  mermaidConfig,
  mermaidCacheKey,
  sanitizeMermaidSvg,
  type MermaidTheme,
} from "./mermaid-policy";

export type MermaidRenderResult =
  | { kind: "success"; svg: string }
  | { kind: "error"; message: string };

const cache = new BoundedLruCache<string, MermaidRenderResult>(MERMAID_CACHE_ENTRIES);
let renderQueue: Promise<void> = Promise.resolve();

export function cachedMermaidResult(
  source: string,
  theme: MermaidTheme,
): MermaidRenderResult | undefined {
  return cache.get(mermaidCacheKey(source, theme));
}

export async function renderMermaid(
  id: string,
  source: string,
  theme: MermaidTheme,
  signal: AbortSignal,
): Promise<MermaidRenderResult> {
  const key = mermaidCacheKey(source, theme);
  const cached = cache.get(key);
  if (cached) return cached;

  try {
    const svg = await withDeadline(
      enqueue(() => renderSvg(id, source, theme)),
      signal,
    );
    const result = { kind: "success", svg } as const;
    cache.set(key, result);
    return result;
  } catch (error) {
    if (signal.aborted) throw error;
    const result = {
      kind: "error",
      message:
        error instanceof MermaidTimeoutError
          ? "Diagram rendering timed out."
          : "Diagram source is invalid.",
    } as const;
    cache.set(key, result);
    return result;
  }
}

async function renderSvg(id: string, source: string, theme: MermaidTheme): Promise<string> {
  const [{ default: mermaid }, { default: createPurifier }] = await Promise.all([
    import("mermaid"),
    import("dompurify"),
  ]);
  mermaid.initialize(mermaidConfig(theme, id));
  const { svg } = await mermaid.render(id, source);
  const purifier = createPurifier(window);
  return sanitizeMermaidSvg(svg, purifier);
}

function enqueue<T>(task: () => Promise<T>): Promise<T> {
  const result = renderQueue.then(task, task);
  renderQueue = result.then(
    () => undefined,
    () => undefined,
  );
  return result;
}

class MermaidTimeoutError extends Error {}

function withDeadline<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    if (signal.aborted) {
      reject(new DOMException("Aborted", "AbortError"));
      return;
    }
    const timeout = window.setTimeout(
      () => finish(() => reject(new MermaidTimeoutError("Mermaid rendering timed out"))),
      MERMAID_RENDER_TIMEOUT_MS,
    );
    const abort = () => finish(() => reject(new DOMException("Aborted", "AbortError")));
    const finish = (complete: () => void) => {
      window.clearTimeout(timeout);
      signal.removeEventListener("abort", abort);
      complete();
    };
    signal.addEventListener("abort", abort, { once: true });
    promise.then(
      (value) => finish(() => resolve(value)),
      (error: unknown) => finish(() => reject(error)),
    );
  });
}
