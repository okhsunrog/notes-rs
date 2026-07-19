export type MathRuntime = {
  rehypeKatex: (typeof import("rehype-katex"))["default"];
  remarkMath: (typeof import("remark-math"))["default"];
};

let runtime: MathRuntime | null = null;
let loading: Promise<MathRuntime> | null = null;
const listeners = new Set<() => void>();

export function markdownMayContainMath(markdown: string): boolean {
  return markdown.includes("$") || /(?:^|\n) {0,3}```math(?:\s|$)/i.test(markdown);
}

export function cachedMathRuntime(): MathRuntime | null {
  return runtime;
}

export function subscribeMathRuntime(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function loadMathRuntime(): Promise<MathRuntime> {
  if (runtime) return Promise.resolve(runtime);
  loading ??= Promise.all([
    import("katex/dist/katex.min.css"),
    import("rehype-katex"),
    import("remark-math"),
  ]).then(([, rehypeKatex, remarkMath]) => {
    runtime = {
      rehypeKatex: rehypeKatex.default,
      remarkMath: remarkMath.default,
    };
    for (const listener of listeners) listener();
    return runtime;
  });
  return loading;
}
