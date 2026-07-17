import { describe, expect, it } from "vite-plus/test";
import {
  BoundedLruCache,
  MAX_MERMAID_SOURCE_CHARS,
  MAX_MERMAID_SOURCE_LINES,
  MermaidRequestController,
  isMermaidFenceLanguage,
  mermaidCacheKey,
  mermaidConfig,
  stableMermaidId,
  validateMermaidSource,
} from "./mermaid-policy";

describe("Mermaid Markdown policy", () => {
  it("recognizes only the explicit Mermaid fence language", () => {
    expect(isMermaidFenceLanguage("mermaid")).toBe(true);
    expect(isMermaidFenceLanguage("Mermaid")).toBe(false);
    expect(isMermaidFenceLanguage("flowchart")).toBe(false);
    expect(isMermaidFenceLanguage(null)).toBe(false);
  });

  it("bounds source by characters and lines", () => {
    expect(validateMermaidSource("flowchart LR\nA --> B")).toEqual({ kind: "valid" });
    expect(validateMermaidSource("x".repeat(MAX_MERMAID_SOURCE_CHARS + 1))).toEqual({
      kind: "oversize",
      reason: "characters",
    });
    expect(validateMermaidSource("x\n".repeat(MAX_MERMAID_SOURCE_LINES))).toEqual({
      kind: "oversize",
      reason: "lines",
    });
  });

  it("uses strict, deterministic and HTML-label-free light/dark configs", () => {
    const light = mermaidConfig("light", "diagram-a");
    const dark = mermaidConfig("dark", "diagram-b");

    expect(light).toMatchObject({
      darkMode: false,
      deterministicIds: true,
      deterministicIDSeed: "diagram-a",
      htmlLabels: false,
      securityLevel: "strict",
      startOnLoad: false,
      suppressErrorRendering: true,
      theme: "default",
    });
    expect(dark).toMatchObject({
      darkMode: true,
      deterministicIDSeed: "diagram-b",
      theme: "dark",
    });
    expect(light.flowchart?.htmlLabels).toBe(false);
    expect(light.secure).toContain("securityLevel");
    expect(light.secure).toContain("htmlLabels");
  });

  it("keys the cache by source, theme and renderer dialect", () => {
    expect(mermaidCacheKey("A --> B", "light")).not.toBe(mermaidCacheKey("A --> B", "dark"));
    expect(mermaidCacheKey("A --> B", "light")).not.toBe(mermaidCacheKey("A --> C", "light"));
    expect(stableMermaidId("same-key", "instance-a")).toBe(
      stableMermaidId("same-key", "instance-a"),
    );
    expect(stableMermaidId("same-key", "instance-a")).not.toBe(
      stableMermaidId("same-key", "instance-b"),
    );
  });

  it("evicts the least-recently-used successful or error result", () => {
    const cache = new BoundedLruCache<string, { kind: "error" | "success" }>(2);
    cache.set("success", { kind: "success" });
    cache.set("error", { kind: "error" });
    expect(cache.get("success")).toEqual({ kind: "success" });
    cache.set("new", { kind: "success" });

    expect(cache.size).toBe(2);
    expect(cache.has("success")).toBe(true);
    expect(cache.has("error")).toBe(false);
    expect(cache.has("new")).toBe(true);
  });

  it("aborts and rejects stale asynchronous requests", () => {
    const controller = new MermaidRequestController();
    const first = controller.start();
    const second = controller.start();

    expect(first.signal.aborted).toBe(true);
    expect(controller.isCurrent(first.id)).toBe(false);
    expect(controller.isCurrent(second.id)).toBe(true);
    controller.cancel(second.id);
    expect(second.signal.aborted).toBe(true);
    expect(controller.isCurrent(second.id)).toBe(false);
  });
});
