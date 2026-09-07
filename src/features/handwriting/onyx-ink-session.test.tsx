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
let resize: ResizeObserverCallback | undefined;
function Sheet({ onInput }: { onInput?: (event: OnyxInkEvent) => void }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [draft, setDraft] = useState(emptyDraft);
  latest = draft;
  const native = useOnyxInk({
    canvasRef,
    enabled: true,
    draft,
    tool: "pen",
    width: 3,
    onInput,
    onChange: setDraft,
    onActiveChange: () => {},
    onMetrics: () => {},
    onLimit: () => {},
  });
  return <canvas ref={canvasRef} data-native={native} />;
}
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  resize = undefined;
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: ResizeObserverCallback) {
        // The canvas observer is created last, so this ends up measuring the sheet.
        resize = callback;
      }
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
function EditorSheet({
  tool,
  lassoMode = "rectangle",
}: {
  tool: InkTool;
  lassoMode?: "free" | "rectangle";
}) {
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
      lassoMode={lassoMode}
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
    canvas: { width: 300, height: 150 },
    getTransform() {
      return { a: 1, d: 1 };
    },
    save() {},
    restore() {},
    rect() {},
    clip() {},
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

it("keeps fast native lasso transient and sends selection bounds for subsequent dragging", async () => {
  const publish = vi.fn();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
    canvas: { width: 300, height: 150 },
    getTransform() {
      return { a: 1, d: 1 };
    },
    save() {},
    restore() {},
    rect() {},
    clip() {},
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
    drawImage: publish,
  } as unknown as CanvasRenderingContext2D);
  await act(async () => root.render(<EditorSheet tool="lasso" lassoMode="free" />));
  const config = bridge.invoke.mock.calls.find(([command]) =>
    command.endsWith("configure_onyx_ink"),
  )![1];
  expect(config).toMatchObject({ fastLasso: true, hasSelection: false });
  const p = (x: number, y: number) => ({ x, y, pressure: 0.5, tiltX: 0, tiltY: 0, time: x });
  const points = [p(0, 80), p(220, 80), p(220, 120), p(0, 120), p(0, 80)];
  const event: OnyxInkEvent = {
    session: config.session,
    sequence: 1,
    kind: "begin",
    width: 3,
    erasing: false,
    fastPreview: true,
    points: [points[0]!],
  };
  publish.mockClear();
  await act(async () => bridge.event(event));
  expect(publish).not.toHaveBeenCalled(); // Native layer owns the live trace.
  await act(async () => {
    bridge.event({ ...event, kind: "stroke", points });
    bridge.event({ ...event, kind: "end" });
  });
  expect(latest.strokes).toHaveLength(1); // Lasso never becomes a stored stroke.
  expect(selectedResult).toEqual(["line"]);
  expect(bridge.invoke).toHaveBeenCalledWith(
    "plugin:mobile-system|configure_onyx_ink",
    expect.objectContaining({
      fastLasso: true,
      hasSelection: true,
      selectionLeft: 0,
      selectionRight: 200,
      selectionTop: 100,
      selectionBottom: 100,
    }),
  );
});

it("coalesces queued previews and discards them when the full stroke arrives", async () => {
  const frames = new Map<number, FrameRequestCallback>();
  let nextFrame = 0;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.set(++nextFrame, callback);
    return nextFrame;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id));
  const flushFrames = () => {
    const pending = [...frames.values()];
    frames.clear();
    for (const callback of pending) callback(0);
  };
  const input = vi.fn();
  await act(async () => root.render(<Sheet onInput={input} />));
  const config = bridge.invoke.mock.calls.find(([command]) =>
    command.endsWith("configure_onyx_ink"),
  )![1];
  const event: OnyxInkEvent = {
    session: config.session,
    kind: "begin",
    sequence: 1,
    erasing: false,
    width: 3,
    points: [{ x: 0, y: 20, pressure: 0.5, tiltX: 0, tiltY: 0, time: 1 }],
  };
  const preview = (x: number): OnyxInkEvent => ({
    ...event,
    kind: "preview",
    points: [{ ...event.points![0]!, x }],
  });
  await act(async () => {
    bridge.event(event);
    for (let x = 1; x <= 100; x++) bridge.event(preview(x));
  });
  expect(input).toHaveBeenCalledTimes(1); // Only begin; 100 positions wait for one frame.
  await act(async () => flushFrames());
  expect(input).toHaveBeenLastCalledWith(preview(100), latest);
  expect(input).toHaveBeenCalledTimes(2);
  await act(async () => {
    bridge.event(preview(101));
    bridge.event({
      ...event,
      kind: "stroke",
      points: [event.points![0]!, preview(200).points![0]!],
    });
    bridge.event({ ...event, kind: "end" });
    bridge.event(preview(102)); // A stale preview after pen-up cannot animate the committed ink.
    flushFrames();
  });
  expect(input.mock.calls.filter(([e]) => e.kind === "preview")).toHaveLength(1);
  expect(latest.strokes[0]!.points[1]!.x).toBe(200);
});

it("starts hardware erasing from newly received ink before React renders the previous batch", async () => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  await act(async () => root.render(<EditorSheet tool="pen" />));
  const { session } = bridge.invoke.mock.calls.find(([command]) =>
    command.endsWith("configure_onyx_ink"),
  )![1];
  const point = { x: 500, y: 500, pressure: 0.7, tiltX: 0, tiltY: 0, time: 1 };
  const event: OnyxInkEvent = {
    session,
    kind: "stroke",
    sequence: 1,
    width: 3,
    erasing: false,
    points: [point],
  };
  await act(async () => {
    bridge.event(event);
    // Flip the pen and erase elsewhere before React has rendered the new dot.
    bridge.event({ ...event, kind: "begin", erasing: true, points: [{ ...point, x: 900 }] });
    bridge.event({ ...event, sequence: 2, erasing: true, points: [{ ...point, x: 900 }] });
    bridge.event({ ...event, sequence: 2, kind: "end", erasing: true });
  });
  expect(latest.strokes).toHaveLength(2);
  expect(latest.strokes[1]!.points).toEqual([point]);
  await act(async () => {
    bridge.event({ ...event, kind: "begin", sequence: 2, erasing: true });
    bridge.event({ ...event, sequence: 3, erasing: true });
    bridge.event({ ...event, sequence: 3, kind: "end", erasing: true });
  });
  expect(latest.strokes).toHaveLength(1);
  expect(latest.strokes[0]!.id).toBe("line");
});

it("hands the floating selection menu to the plugin as an overlay, never as a hole in the region", async () => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  vi.spyOn(HTMLDivElement.prototype, "getBoundingClientRect").mockReturnValue({
    width: 120,
    height: 40,
  } as DOMRect);
  await act(async () => root.render(<EditorSheet tool="lasso" />));
  await act(async () =>
    resize?.(
      [{ contentRect: { width: 500, height: 700 } } as ResizeObserverEntry],
      {} as ResizeObserver,
    ),
  );
  const configs = () =>
    bridge.invoke.mock.calls.filter(([command]) => command.endsWith("configure_onyx_ink"));
  const config = configs()[0]![1];
  expect(config.overlayRects).toEqual([]); // Nothing selected, nothing over the sheet.
  expect(config).not.toHaveProperty("excludeRects"); // The firmware region is never punched.
  const p = (x: number, y: number) => ({ x, y, pressure: 0.5, tiltX: 0, tiltY: 0, time: x });
  const event: OnyxInkEvent = {
    session: config.session,
    sequence: 1,
    kind: "begin",
    width: 3,
    erasing: false,
    points: [p(0, 80)],
  };
  await act(async () => {
    bridge.event(event);
    bridge.event({ ...event, kind: "stroke", points: [p(0, 80), p(220, 120)] });
    bridge.event({ ...event, kind: "end" });
  });
  expect(selectedResult).toEqual(["line"]);
  // Canvas origin (12, 80) plus the menu's place on the sheet, in the plugin's own CSS pixels.
  expect(configs().pop()![1].overlayRects).toEqual([
    { left: 20, top: 138, width: 120, height: 40 },
  ]);
});

it("leaves the drawing region alone while a lasso gesture is open", async () => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  vi.spyOn(HTMLDivElement.prototype, "getBoundingClientRect").mockReturnValue({
    width: 120,
    height: 40,
  } as DOMRect);
  await act(async () => root.render(<EditorSheet tool="lasso" />));
  await act(async () =>
    resize?.(
      [{ contentRect: { width: 500, height: 700 } } as ResizeObserverEntry],
      {} as ResizeObserver,
    ),
  );
  const configs = () =>
    bridge.invoke.mock.calls.filter(([command]) => command.endsWith("configure_onyx_ink"));
  const session = configs()[0]![1].session as string;
  const p = (x: number, y: number) => ({ x, y, pressure: 0.5, tiltX: 0, tiltY: 0, time: x });
  const event: OnyxInkEvent = {
    session,
    sequence: 1,
    kind: "begin",
    width: 3,
    erasing: false,
    points: [p(0, 80)],
  };
  await act(async () => {
    bridge.event(event);
    bridge.event({ ...event, kind: "stroke", points: [p(0, 80), p(220, 120)] });
    bridge.event({ ...event, kind: "end" });
  });
  expect(configs().pop()![1].overlayRects).toHaveLength(1);
  // A second lasso hides the menu, but reconfiguring mid-gesture would pause raw drawing and
  // truncate the trace, so nothing reaches the plugin until the pen lifts.
  const pending = configs().length;
  await act(async () => {
    bridge.event({ ...event, sequence: 2, points: [p(600, 800)] });
  });
  expect(container.querySelector("[data-ink-selection-menu]")).toBeNull();
  expect(configs()).toHaveLength(pending);
  await act(async () => {
    bridge.event({ ...event, sequence: 2, kind: "stroke", points: [p(600, 800), p(800, 900)] });
    bridge.event({ ...event, sequence: 2, kind: "end" });
  });
  expect(selectedResult).toEqual([]);
  expect(configs().pop()![1].overlayRects).toEqual([]);
});
