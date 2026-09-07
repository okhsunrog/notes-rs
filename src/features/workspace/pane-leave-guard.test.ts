import { afterEach, expect, it, vi } from "vite-plus/test";
import { mayLeavePane, registerPaneLeaveGuard, resetPaneLeaveGuards } from "./pane-leave-guard";
import type { PaneId } from "./workspace-model";

const pane = "pane-1" as PaneId;

afterEach(() => resetPaneLeaveGuards());

it("lets a pane without a guard leave", async () => {
  expect(await mayLeavePane(pane)).toBe(true);
});

it("asks the registered guard and honours its answer", async () => {
  const guard = vi.fn().mockResolvedValue(false);
  const unregister = registerPaneLeaveGuard(pane, guard);
  expect(await mayLeavePane(pane)).toBe(false);
  guard.mockResolvedValue(true);
  expect(await mayLeavePane(pane)).toBe(true);
  unregister();
  guard.mockResolvedValue(false);
  expect(await mayLeavePane(pane)).toBe(true);
});

it("keeps the pane when the guard throws and ignores a stale unregister", async () => {
  const first = () => {
    throw new Error("boom");
  };
  const unregisterFirst = registerPaneLeaveGuard(pane, first);
  registerPaneLeaveGuard(pane, () => true);
  unregisterFirst();
  expect(await mayLeavePane(pane)).toBe(true);
  registerPaneLeaveGuard(pane, first);
  expect(await mayLeavePane(pane)).toBe(false);
});
