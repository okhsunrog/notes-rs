// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vite-plus/test";
import { useAppShortcuts } from "./use-app-shortcuts";

const actions = {
  createNote: vi.fn(),
  openSearch: vi.fn(),
  undo: vi.fn(),
  redo: vi.fn(),
};

let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  for (const action of Object.values(actions)) action.mockReset();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

function Harness({ historyEnabled }: { historyEnabled: boolean }) {
  useAppShortcuts({ enabled: true, historyEnabled, ...actions });
  return <textarea aria-label="draft" />;
}

async function mount(historyEnabled = true) {
  await act(async () => root.render(<Harness historyEnabled={historyEnabled} />));
}

function press(key: string, target: EventTarget) {
  target.dispatchEvent(
    new KeyboardEvent("keydown", { key, ctrlKey: true, bubbles: true, cancelable: true }),
  );
}

it("leaves every shortcut to the text field the caret is in", async () => {
  await mount();
  const field = container.querySelector("textarea");
  if (!field) throw new Error("no field");

  for (const key of ["k", "n", "z"]) press(key, field);

  expect(actions.openSearch).not.toHaveBeenCalled();
  expect(actions.createNote).not.toHaveBeenCalled();
  expect(actions.undo).not.toHaveBeenCalled();
});

it("still handles the shortcuts outside a text field", async () => {
  await mount();

  press("k", window);
  press("n", window);
  press("z", window);

  expect(actions.openSearch).toHaveBeenCalledTimes(1);
  expect(actions.createNote).toHaveBeenCalledTimes(1);
  expect(actions.undo).toHaveBeenCalledTimes(1);
});

it("gives up only undo and redo while the handwriting editor owns them", async () => {
  await mount(false);

  press("k", window);
  press("n", window);
  press("z", window);

  expect(actions.openSearch).toHaveBeenCalledTimes(1);
  expect(actions.createNote).toHaveBeenCalledTimes(1);
  expect(actions.undo).not.toHaveBeenCalled();
});
