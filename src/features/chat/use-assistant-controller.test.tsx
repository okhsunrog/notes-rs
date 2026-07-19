// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  CHAT_INACTIVITY_TIMEOUT_MS,
  useAssistantController,
  type AssistantController,
} from "./use-assistant-controller";

const transport = vi.hoisted(() => ({
  cancelChat: vi.fn(),
  channel: null as { onmessage: (event: unknown) => void } | null,
  chatStream: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage = (_event: unknown) => undefined;

    constructor() {
      transport.channel = this;
    }
  },
}));

vi.mock("@/app/confirmation", () => ({ useConfirmation: () => vi.fn(async () => true) }));
vi.mock("@/lib/api", () => ({
  cancelChat: transport.cancelChat,
  chatStream: transport.chatStream,
}));

let controller: AssistantController | null = null;
let unmount: (() => void) | null = null;

beforeAll(() => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  vi.useFakeTimers();
  transport.cancelChat.mockReset();
  transport.chatStream.mockReset();
  transport.chatStream.mockImplementation(() => new Promise<string>(() => undefined));
  transport.channel = null;
  controller = null;
  window.localStorage.clear();
});

afterEach(() => {
  unmount?.();
  unmount = null;
  vi.useRealTimers();
});

describe("assistant stream inactivity", () => {
  it("extends the deadline on every event and fails a silent stream", async () => {
    await renderController();
    await startRequest();
    const requestId = controller?.state.activeRequestId;
    expect(requestId).not.toBeNull();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(CHAT_INACTIVITY_TIMEOUT_MS - 1_000);
      transport.channel?.onmessage({ kind: "text_delta", text: "still working" });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(CHAT_INACTIVITY_TIMEOUT_MS - 1);
    });
    expect(controller?.state.busy).toBe(true);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(controller?.state.busy).toBe(false);
    const turns = controller?.state.turns ?? [];
    expect(turns[turns.length - 1]?.text).toContain(
      "AI stream timed out after 60 seconds without activity.",
    );
    expect(transport.cancelChat).toHaveBeenCalledWith(requestId);
  });

  it("cancels the inactivity deadline when the user cancels", async () => {
    await renderController();
    await startRequest();

    act(() => controller?.cancel());
    await act(async () => {
      await vi.advanceTimersByTimeAsync(CHAT_INACTIVITY_TIMEOUT_MS);
    });

    expect(transport.cancelChat).toHaveBeenCalledTimes(1);
  });
});

async function renderController() {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  function Probe() {
    controller = useAssistantController();
    return null;
  }
  await act(async () => root.render(<Probe />));
  unmount = () => {
    act(() => root.unmount());
    container.remove();
  };
}

async function startRequest() {
  act(() => controller?.setInput("question"));
  await act(async () => {
    void controller?.send(null);
    await Promise.resolve();
  });
  expect(controller?.state.busy).toBe(true);
}
