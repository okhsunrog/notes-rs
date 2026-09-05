// @vitest-environment jsdom
import { act, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { InkDraft } from "@/lib/bindings";
import { InkCanvas, type InkTool } from "./ink-canvas";
import { emptyDraft } from "./ink-model";
import { useOnyxInk, type OnyxInkEvent } from "./onyx-ink";

const bridge = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  unregister: vi.fn(),
  event: (_event: OnyxInkEvent) => {},
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: bridge.invoke,
  addPluginListener: bridge.listen,
}));
let root: ReturnType<typeof createRoot>;
let container: HTMLDivElement;
let latest: InkDraft;
function Sheet() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [draft, setDraft] = useState(emptyDraft);
  latest = draft;
  const native = useOnyxInk({
    canvasRef,
    enabled: true,
    draft,
    tool: "pen",
    width: 3,
    onChange: setDraft,
    onActiveChange: () => {},
    onMetrics: () => {},
    onLimit: () => {},
  });
  return <canvas ref={canvasRef} data-native={native} />;
}
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  vi.stubGlobal("requestAnimationFrame", (callback: () => void) => setTimeout(callback, 0));
  vi.stubGlobal("cancelAnimationFrame", clearTimeout);
  vi.spyOn(HTMLCanvasElement.prototype, "getBoundingClientRect").mockReturnValue({
    left: 12,
    top: 80,
    width: 500,
    height: 700,
    bottom: 780,
  } as DOMRect);
  bridge.invoke.mockReset().mockResolvedValue({ available: true, active: true });
  bridge.unregister.mockReset().mockResolvedValue(undefined);
  bridge.listen.mockReset().mockImplementation((_plugin, _name, callback) => {
    bridge.event = callback;
    return Promise.resolve({ unregister: bridge.unregister });
  });
  container = document.createElement("div");
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("keeps consecutive batches received before a render, deduplicates them and rejects old sessions", async () => {
  await act(async () => root.render(<Sheet />));
  const config = bridge.invoke.mock.calls.find(([command]) =>
    command.endsWith("configure_onyx_ink"),
  )![1];
  expect(config).toMatchObject({ left: 12, top: 80, width: 500, height: 700 });
  expect(container.querySelector("canvas")!.dataset.native).toBe("true");
  const event: OnyxInkEvent = {
    session: config.session,
    kind: "stroke",
    sequence: 1,
    width: 3,
    erasing: false,
    points: [{ x: 10, y: 20, pressure: 0.7, tiltX: 0, tiltY: 0, time: 1 }],
  };
  await act(async () => {
    bridge.event(event);
    bridge.event({ ...event, sequence: 2 });
    bridge.event({ ...event, sequence: 2 });
    bridge.event({ ...event, sequence: 3, session: "closed-sheet" });
  });
  expect(latest.strokes).toHaveLength(2);
  await act(async () => root.render(null));
  expect(bridge.unregister).toHaveBeenCalledTimes(1);
  expect(bridge.invoke).toHaveBeenCalledWith("plugin:mobile-system|configure_onyx_ink", {
    session: config.session,
    enabled: false,
  });
});

it("falls back to pointer input when the device has no BOOX SDK support", async () => {
  bridge.invoke.mockResolvedValue({ available: false, active: false });
  await act(async () => root.render(<Sheet />));
  expect(container.querySelector("canvas")!.dataset.native).toBe("false");
});

let selectedResult: string[] = [];
function EditorSheet({ tool }: { tool: InkTool }) {
  const [draft, setDraft] = useState(() => ({
    ...emptyDraft(),
    strokes: [
      {
        id: "line",
        width: 2,
        points: [
          { x: 0, y: 100, pressure: 0.5, tiltX: 0, tiltY: 0, time: 0 },
          { x: 200, y: 100, pressure: 0.5, tiltX: 0, tiltY: 0, time: 200 },
        ],
      },
    ],
  }));
  const [selected, setSelected] = useState<string[]>([]);
  latest = draft;
  selectedResult = selected;
  return (
    <InkCanvas
      draft={draft}
      tool={tool}
      width={3}
      nativeInk
      mouseEnabled={false}
      eraserMode="pixel"
      eraserRadius={9}
      lassoMode="rectangle"
      selected={selected}
      onSelectionChange={setSelected}
      onChange={setDraft}
      onActiveChange={() => {}}
      onMetrics={() => {}}
      onLimit={() => {}}
    />
  );
}
it("routes native hardware erasing and lasso gestures to the shared editor without adding ink", async () => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
    setTransform() {},
    clearRect() {},
    fillRect() {},
    setLineDash() {},
    beginPath() {},
    arc() {},
    fill() {},
    moveTo() {},
    lineTo() {},
    stroke() {},
    drawImage() {},
  } as unknown as CanvasRenderingContext2D);
  await act(async () => root.render(<EditorSheet tool="pen" />));
  const config = bridge.invoke.mock.calls.find(([command]) =>
    command.endsWith("configure_onyx_ink"),
  )![1];
  const p = (x: number, y: number) => ({ x, y, pressure: 0.5, tiltX: 0, tiltY: 0, time: x });
  const base: OnyxInkEvent = {
    session: config.session,
    sequence: 1,
    kind: "begin",
    width: 3,
    erasing: true,
    points: [p(100, 50)],
  };
  await act(async () => {
    bridge.event(base);
    bridge.event({ ...base, kind: "preview", points: [p(100, 150)] });
  });
  expect(latest.strokes).toHaveLength(1); // Preview is not saved.
  await act(async () => {
    bridge.event({ ...base, kind: "stroke", points: [p(100, 50), p(100, 150)] });
    bridge.event({ ...base, kind: "end" });
  });
  expect(latest.strokes).toHaveLength(2);
  await act(async () => root.render(<EditorSheet tool="lasso" />));
  expect(bridge.invoke.mock.calls.some(([, args]) => args.interaction === true)).toBe(true);
  await act(async () => {
    bridge.event({ ...base, sequence: 2, erasing: false, points: [p(0, 80)] });
    bridge.event({
      ...base,
      sequence: 2,
      erasing: false,
      kind: "stroke",
      points: [p(0, 80), p(95, 120)],
    });
    bridge.event({ ...base, sequence: 2, kind: "end" });
  });
  expect(latest.strokes).toHaveLength(2);
  expect(selectedResult).toEqual(["line"]);
});
