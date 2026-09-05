// @vitest-environment jsdom
import { act } from "react";
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

function renderDraft(draft: InkDraft) {
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
        beginPath: vi.fn(),
        arc: vi.fn(),
        fill: vi.fn(),
        moveTo: vi.fn(),
        lineTo: vi.fn(),
        stroke: vi.fn(),
        clearRect: vi.fn(),
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
