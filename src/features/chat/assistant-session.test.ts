import { describe, expect, it } from "vitest";
import { assistantSessionReducer, createAssistantSession } from "./assistant-session";

describe("Assistant session controller state", () => {
  it("preserves input independently from any dock host", () => {
    const state = assistantSessionReducer(createAssistantSession(), {
      type: "set_input",
      input: "draft question",
    });

    expect(state.input).toBe("draft question");
    expect(state.busy).toBe(false);
  });

  it("continues applying stream events while its view is absent", () => {
    let state = assistantSessionReducer(createAssistantSession(), {
      type: "start",
      message: "question",
      requestId: "request-1",
    });
    state = assistantSessionReducer(state, {
      type: "event",
      event: { kind: "text_delta", text: "answer" },
    });

    expect(state.busy).toBe(true);
    expect(state.activeRequestId).toBe("request-1");
    expect(state.turns[state.turns.length - 1]?.text).toBe("answer");
  });

  it("ignores a stale completion and retains cancellation output", () => {
    let state = assistantSessionReducer(createAssistantSession(), {
      type: "start",
      message: "question",
      requestId: "request-1",
    });
    state = assistantSessionReducer(state, { type: "finish", requestId: "stale" });
    expect(state.busy).toBe(true);

    state = assistantSessionReducer(state, {
      type: "event",
      event: { kind: "cancelled" },
    });
    state = assistantSessionReducer(state, { type: "finish", requestId: "request-1" });
    expect(state.busy).toBe(false);
    expect(state.turns[state.turns.length - 1]?.text).toContain("Stopped");
  });
});
