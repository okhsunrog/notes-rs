import type { ChatEvent, ChatTurn } from "@/lib/api";

export type AssistantSessionState = Readonly<{
  input: string;
  turns: readonly ChatTurn[];
  busy: boolean;
  allowWrites: boolean;
  activeRequestId: string | null;
}>;

export type AssistantSessionAction =
  | { readonly type: "set_input"; readonly input: string }
  | { readonly type: "set_allow_writes"; readonly allow: boolean }
  | { readonly type: "start"; readonly message: string; readonly requestId: string }
  | { readonly type: "event"; readonly event: ChatEvent }
  | { readonly type: "fail"; readonly message: string }
  | { readonly type: "finish"; readonly requestId: string }
  | { readonly type: "clear" };

export function createAssistantSession(turns: readonly ChatTurn[] = []): AssistantSessionState {
  return {
    input: "",
    turns: [...turns],
    busy: false,
    allowWrites: false,
    activeRequestId: null,
  };
}

export function assistantSessionReducer(
  state: AssistantSessionState,
  action: AssistantSessionAction,
): AssistantSessionState {
  switch (action.type) {
    case "set_input":
      return { ...state, input: action.input };
    case "set_allow_writes":
      return { ...state, allowWrites: action.allow };
    case "start":
      return {
        ...state,
        input: "",
        turns: [
          ...state.turns,
          { role: "user", text: action.message },
          { role: "assistant", text: "", tools: [] },
        ],
        busy: true,
        activeRequestId: action.requestId,
      };
    case "event":
      return { ...state, turns: applyAssistantEvent(state.turns, action.event) };
    case "fail":
      return {
        ...state,
        turns: replaceLastAssistant(state.turns, {
          role: "assistant",
          text: `error: ${action.message}`,
        }),
      };
    case "finish":
      if (state.activeRequestId !== action.requestId) return state;
      return {
        ...state,
        busy: false,
        allowWrites: false,
        activeRequestId: null,
      };
    case "clear":
      return state.busy ? state : { ...state, turns: [] };
  }
}

export function boundedAssistantHistory(turns: readonly ChatTurn[]) {
  const result: Array<{ role: "user" | "assistant"; text: string }> = [];
  let characters = 0;
  for (const turn of [...turns].reverse()) {
    const size = turn.text.length;
    if (result.length >= 24 || characters + size > 32_000) break;
    result.push({ role: turn.role, text: turn.text });
    characters += size;
  }
  return result.reverse();
}

function applyAssistantEvent(turns: readonly ChatTurn[], event: ChatEvent): readonly ChatTurn[] {
  const last = turns[turns.length - 1];
  if (!last || last.role !== "assistant") return turns;
  const updated = { ...last };
  switch (event.kind) {
    case "text_delta":
      updated.text += event.text;
      break;
    case "tool_start":
      updated.tools = [
        ...(updated.tools ?? []),
        { id: event.id, name: event.name, args: event.args },
      ];
      break;
    case "tool_end":
      updated.tools = (updated.tools ?? []).map((tool) =>
        tool.id === event.id ? { ...tool, result: event.result } : tool,
      );
      break;
    case "usage":
      updated.usage = {
        inputTokens: event.inputTokens,
        outputTokens: event.outputTokens,
        totalTokens: event.totalTokens,
      };
      break;
    case "error":
      updated.text = `error: ${event.message}`;
      break;
    case "cancelled":
      updated.text = `${updated.text}\n\n_Stopped._`;
      break;
  }
  return [...turns.slice(0, -1), updated];
}

function replaceLastAssistant(
  turns: readonly ChatTurn[],
  replacement: ChatTurn,
): readonly ChatTurn[] {
  const last = turns[turns.length - 1];
  if (!last || last.role !== "assistant") return [...turns, replacement];
  return [...turns.slice(0, -1), replacement];
}
