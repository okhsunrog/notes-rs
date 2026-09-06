// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Dialog } from "@/components/ui/dialog";
import {
  installTextFocusSuppression,
  resetInkSuppression,
  setInkSuppressionSupported,
  suppressOnyxInk,
} from "./ink-suppression";

const bridge = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: bridge.invoke }));

const SUPPRESS = "plugin:mobile-system|suppress_onyx_ink";
const calls = () =>
  bridge.invoke.mock.calls
    .filter(([command]) => command === SUPPRESS)
    .map(([, args]) => args as { reason: string; active: boolean });

beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  bridge.invoke.mockReset().mockResolvedValue({ paused: [] });
  resetInkSuppression();
  setInkSuppressionSupported(true);
});
afterEach(() => {
  resetInkSuppression();
  vi.useRealTimers();
});

it("holds the pen while the caret is in a field and releases it when focus leaves", async () => {
  vi.useFakeTimers();
  const stop = installTextFocusSuppression();
  const input = document.createElement("input");
  const other = document.createElement("textarea");
  const button = document.createElement("button");
  document.body.append(input, other, button);

  input.focus();
  expect(calls()).toEqual([{ reason: "text-focus", active: true }]);

  // Moving between two fields keeps the keyboard up, so the pen must stay down.
  input.blur();
  other.focus();
  await vi.advanceTimersByTimeAsync(1);
  expect(calls()).toEqual([{ reason: "text-focus", active: true }]);

  other.blur();
  button.focus();
  await vi.advanceTimersByTimeAsync(1);
  expect(calls()).toEqual([
    { reason: "text-focus", active: true },
    { reason: "text-focus", active: false },
  ]);

  stop();
  input.remove();
  other.remove();
  button.remove();
});

it("never asks the plugin on a device without it, and pushes what is open once it answers", () => {
  resetInkSuppression();
  suppressOnyxInk("overlay:1", true);
  expect(calls()).toEqual([]);
  setInkSuppressionSupported(true);
  expect(calls()).toEqual([{ reason: "overlay:1", active: true }]);
  // The same reason twice is one pause and takes one release.
  suppressOnyxInk("overlay:1", true);
  suppressOnyxInk("overlay:1", false);
  suppressOnyxInk("overlay:1", false);
  expect(calls()).toEqual([
    { reason: "overlay:1", active: true },
    { reason: "overlay:1", active: false },
  ]);
});

it("pauses for an open dialog and resumes after it closes", async () => {
  vi.useFakeTimers();
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);

  await act(async () => root.render(<Dialog open={false} />));
  await act(async () => vi.advanceTimersByTimeAsync(400));
  expect(calls()).toEqual([]);

  await act(async () => root.render(<Dialog open />));
  expect(calls()).toHaveLength(1);
  expect(calls()[0]).toMatchObject({ active: true });
  const reason = calls()[0]!.reason;

  await act(async () => root.render(<Dialog open={false} />));
  // The overlay is still on the panel for a moment after React drops it.
  expect(calls()).toHaveLength(1);
  await act(async () => vi.advanceTimersByTimeAsync(400));
  expect(calls()).toEqual([
    { reason, active: true },
    { reason, active: false },
  ]);

  await act(async () => root.unmount());
  container.remove();
});

it("releases an overlay that is unmounted while it is still open", async () => {
  vi.useFakeTimers();
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);

  await act(async () => root.render(<Dialog open />));
  const reason = calls()[0]!.reason;
  await act(async () => root.unmount());
  expect(calls()).toEqual([
    { reason, active: true },
    { reason, active: false },
  ]);
  container.remove();
});
