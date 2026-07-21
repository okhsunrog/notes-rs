import { describe, expect, it } from "vite-plus/test";
import {
  adjacentDisposition,
  allNotesTarget,
  createInitialWorkspaceState,
  currentDisposition,
  pageTarget,
  workspaceReducer,
} from "./workspace-model";
import { decodeWorkspaceSession, encodeWorkspaceSession } from "./workspace-session";

describe("workspace session", () => {
  it("round-trips a valid two-pane session", () => {
    const initial = createInitialWorkspaceState();
    const state = workspaceReducer(initial, {
      type: "open_target",
      target: pageTarget("019cfa51-8d73-7b53-b090-cdb945bb1b4d"),
      disposition: adjacentDisposition,
    });

    expect(decodeWorkspaceSession(encodeWorkspaceSession(state))).toEqual(state);
  });

  it("round-trips the All Notes catalog target", () => {
    const state = workspaceReducer(createInitialWorkspaceState(), {
      type: "open_target",
      target: allNotesTarget,
      disposition: currentDisposition,
    });

    expect(decodeWorkspaceSession(encodeWorkspaceSession(state))).toEqual(state);
  });

  it("rejects malformed and invariant-breaking sessions", () => {
    expect(decodeWorkspaceSession("not json")).toBeNull();
    expect(decodeWorkspaceSession('{"version":2,"state":{}}')).toBeNull();

    const state = createInitialWorkspaceState();
    expect(
      decodeWorkspaceSession(
        JSON.stringify({
          version: 1,
          state: { ...state, activePaneId: "missing-pane" },
        }),
      ),
    ).toBeNull();
  });
});
