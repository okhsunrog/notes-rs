// @vitest-environment jsdom
import { act, type ComponentProps } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { InkCanvas } from "./ink-canvas";
import { emptyDraft } from "./ink-model";
import type { InkDraft } from "@/lib/bindings";

let root: ReturnType<typeof createRoot>;
let container: HTMLDivElement;
let canvas: HTMLCanvasElement;
const changed = vi.fn();
let resize: ResizeObserverCallback;
let contexts: WeakMap<HTMLCanvasElement, CanvasRenderingContext2D>;

function renderDraft(draft: InkDraft, props: Partial<ComponentProps<typeof InkCanvas>> = {}) {
  root.render(
    <InkCanvas
      draft={draft}
      tool="pen"
      width={3}
      mouseEnabled={false}
      onChange={changed}
      onActiveChange={vi.fn()}
      onMetrics={vi.fn()}
      onLimit={vi.fn()}
      {...props}
    />,
  );
}

beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: ResizeObserverCallback) {
        resize = callback;
      }
      observe() {}
      disconnect() {}
    },
  );
  contexts = new WeakMap();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
    function (this: HTMLCanvasElement) {
      let context = contexts.get(this);
      if (context) return context;
      context = {
        setLineDash: vi.fn(),
        beginPath: vi.fn(),
        arc: vi.fn(),
        fill: vi.fn(),
        moveTo: vi.fn(),
        lineTo: vi.fn(),
        stroke: vi.fn(),
        clearRect: vi.fn(),
        fillRect: vi.fn(),
        setTransform: vi.fn(),
        drawImage: vi.fn(),
      } as unknown as CanvasRenderingContext2D;
      contexts.set(this, context);
      return context;
    },
  );
  container = document.createElement("div");
  root = createRoot(container);
  changed.mockClear();
  act(() =>
    root.render(
      <InkCanvas
        draft={emptyDraft()}
        tool="pen"
        width={3}
        mouseEnabled={false}
        onChange={changed}
        onActiveChange={vi.fn()}
        onMetrics={vi.fn()}
        onLimit={vi.fn()}
      />,
    ),
  );
  canvas = container.querySelector("canvas")!;
  canvas.setPointerCapture = vi.fn();
  canvas.releasePointerCapture = vi.fn();
  canvas.hasPointerCapture = () => true;
  vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({
    left: 0,
    top: 0,
    width: 500,
    height: 700,
  } as DOMRect);
});

it("publishes complete replacement images without resizing or clearing the visible canvas", () => {
  act(() =>
    resize(
      [{ contentRect: { width: 500, height: 700 } } as ResizeObserverEntry],
      {} as ResizeObserver,
    ),
  );
  const context = contexts.get(canvas)!;
  const clear = vi.spyOn(context, "clearRect");
  const publish = vi.spyOn(context, "drawImage");
  const setWidth = vi.spyOn(canvas, "width", "set");
  const setHeight = vi.spyOn(canvas, "height", "set");
  const draft: InkDraft = {
    ...emptyDraft(),
    strokes: [
      {
        id: "stroke",
        width: 3,
        points: [{ x: 10, y: 20, pressure: 0.5, tiltX: 0, tiltY: 0, time: 1 }],
      },
    ],
  };
  act(() => renderDraft(draft));
  act(() => renderDraft(emptyDraft())); // Undo also replaces the image without a visible clear.
  expect(setWidth).not.toHaveBeenCalled();
  expect(setHeight).not.toHaveBeenCalled();
  expect(clear).not.toHaveBeenCalled();
  expect(publish).toHaveBeenCalledTimes(3);
  expect(context.globalCompositeOperation).toBe("source-over");
});
afterEach(() => {
  act(() => root.unmount());
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function pointer(type: string, overrides: Record<string, unknown> = {}) {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.assign(event, {
    pointerId: 1,
    pointerType: "pen",
    clientX: 20,
    clientY: 30,
    pressure: 0.7,
    tiltX: 10,
    tiltY: 15,
    buttons: 1,
    button: 0,
    ...overrides,
  });
  act(() => {
    canvas.dispatchEvent(event);
  });
}

it("ignores palm touches and preserves pen pressure after lift", () => {
  pointer("pointerdown", { pointerType: "touch" });
  pointer("pointerup", { pointerType: "touch" });
  expect(changed).not.toHaveBeenCalled();
  pointer("pointerdown");
  pointer("pointermove", { clientX: 80, pressure: 0.9 });
  pointer("pointerup", { clientX: 90, pressure: 0 });
  expect(changed).toHaveBeenCalledTimes(1);
  const points = changed.mock.calls[0]![0].strokes[0].points;
  expect(points[0]).toMatchObject({ x: 40, y: 60, pressure: 0.7 });
  expect(points[points.length - 1]).toMatchObject({ x: 180, pressure: 0.9 });
});

it("discards cancelled strokes and ignores a second pointer while writing", () => {
  pointer("pointerdown");
  pointer("pointerdown", { pointerId: 2, pointerType: "touch" });
  pointer("pointerup", { pointerId: 2, pointerType: "touch" });
  expect(changed).not.toHaveBeenCalled();
  pointer("pointercancel");
  expect(changed).not.toHaveBeenCalled();
  pointer("pointerdown");
  pointer("pointerup");
  expect(changed).toHaveBeenCalledTimes(1);
});

it("selects with a rectangle and moves selected strokes as one undoable gesture", () => {
  const draft = {
    ...emptyDraft(),
    strokes: [
      {
        id: "a",
        width: 2,
        points: [
          { x: 50, y: 60, pressure: 0.5, tiltX: 0, tiltY: 0, time: 1 },
          { x: 100, y: 80, pressure: 0.6, tiltX: 0, tiltY: 0, time: 2 },
        ],
      },
    ],
  };
  const selection = vi.fn();
  act(() =>
    renderDraft(draft, { tool: "lasso", lassoMode: "rectangle", onSelectionChange: selection }),
  );
  pointer("pointerdown", { clientX: 10, clientY: 10 });
  pointer("pointermove", { clientX: 80, clientY: 60 });
  pointer("pointerup", { clientX: 80, clientY: 60 });
  expect(selection).toHaveBeenLastCalledWith(["a"]);
  expect(changed).not.toHaveBeenCalled();
  act(() => renderDraft(draft, { tool: "lasso", selected: ["a"], onSelectionChange: selection }));
  pointer("pointerdown", { clientX: 30, clientY: 35 });
  pointer("pointermove", { clientX: 40, clientY: 50 });
  expect(container.querySelector("svg")).toBeNull();
  const visible = contexts.get(canvas)!;
  const published = vi.spyOn(visible, "drawImage").mock.calls;
  const buffer = published[published.length - 1]![0] as HTMLCanvasElement;
  const staging = contexts.get(buffer)!;
  expect(vi.spyOn(staging, "moveTo")).toHaveBeenCalledWith(64, 84); // Selection at the moved ink bounds.
  expect(vi.spyOn(visible, "lineTo")).not.toHaveBeenCalled(); // Both are published via one bitmap.
  pointer("pointerup", { clientX: 40, clientY: 50 });
  expect(changed).toHaveBeenCalledTimes(1);
  expect(changed.mock.calls[0]![0].strokes[0].points[0]).toMatchObject({ x: 70, y: 90 });
});

it("hardware eraser uses the selected pixel mode and cancel leaves handwriting intact", () => {
  const draft = {
    ...emptyDraft(),
    background: "grid" as const,
    strokes: [
      {
        id: "a",
        width: 2,
        points: [
          { x: 0, y: 100, pressure: 0.5, tiltX: 0, tiltY: 0, time: 1 },
          { x: 200, y: 100, pressure: 0.5, tiltX: 0, tiltY: 0, time: 2 },
        ],
      },
    ],
  };
  act(() => renderDraft(draft, { eraserMode: "pixel", eraserRadius: 9 }));
  pointer("pointerdown", { button: 5, buttons: 32, clientX: 50, clientY: 50 });
  pointer("pointercancel");
  expect(changed).not.toHaveBeenCalled();
  pointer("pointerdown", { button: 5, buttons: 32, clientX: 50, clientY: 50 });
  pointer("pointerup", { button: 5, buttons: 0, clientX: 50, clientY: 50 });
  expect(changed).toHaveBeenCalledTimes(1);
  expect(changed.mock.calls[0]![0].strokes).toHaveLength(2);
  expect(changed.mock.calls[0]![0].background).toBe("grid");
});

it("reuses the sheet bitmap during a contour and rebuilds it when paper changes", () => {
  act(() =>
    resize(
      [{ contentRect: { width: 500, height: 700 } } as ResizeObserverEntry],
      {} as ResizeObserver,
    ),
  );
  const draft = emptyDraft();
  act(() => renderDraft(draft, { tool: "lasso" }));
  const visible = contexts.get(canvas)!;
  const publishes = vi.spyOn(visible, "drawImage");
  const buffer = publishes.mock.calls[publishes.mock.calls.length - 1]![0] as HTMLCanvasElement;
  const staging = contexts.get(buffer)!;
  const stages = vi.spyOn(staging, "drawImage").mock.calls;
  const scene = stages[stages.length - 1]![0] as HTMLCanvasElement;
  const redraw = vi.spyOn(contexts.get(scene)!, "fillRect");
  redraw.mockClear();
  pointer("pointerdown");
  pointer("pointermove", { clientX: 80, clientY: 80 });
  pointer("pointermove", { clientX: 100, clientY: 100 });
  pointer("pointerup", { clientX: 100, clientY: 100 });
  expect(redraw).not.toHaveBeenCalled();
  act(() => renderDraft({ ...draft, background: "grid" }, { tool: "lasso" }));
  expect(redraw).toHaveBeenCalledTimes(1);
});
