import { useCallback, useEffect, useReducer, useRef } from "react";
import { Channel } from "@tauri-apps/api/core";
import { useConfirmation } from "@/app/confirmation";
import { cancelChat, chatStream, type ChatEvent, type ChatTurn } from "@/lib/api";
import { DebouncedAction } from "@/lib/debounced-action";
import {
  assistantSessionReducer,
  boundedAssistantHistory,
  createAssistantSession,
} from "./assistant-session";

const CHAT_STORAGE_KEY = "notes-rs.chat.v1";
export const CHAT_INACTIVITY_TIMEOUT_MS = 60_000;

function loadStoredChat(): ChatTurn[] {
  try {
    const value = JSON.parse(localStorage.getItem(CHAT_STORAGE_KEY) ?? "[]");
    return Array.isArray(value) ? value.slice(-50) : [];
  } catch {
    return [];
  }
}

export function useAssistantController() {
  const confirm = useConfirmation();
  const [state, dispatch] = useReducer(assistantSessionReducer, undefined, () =>
    createAssistantSession(loadStoredChat()),
  );
  const stateRef = useRef(state);
  stateRef.current = state;
  const inactivityTimer = useRef(new DebouncedAction());
  const inactivityRequestId = useRef<string | null>(null);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      localStorage.setItem(CHAT_STORAGE_KEY, JSON.stringify(state.turns.slice(-50)));
    }, 200);
    return () => window.clearTimeout(timer);
  }, [state.turns]);

  useEffect(
    () => () => {
      inactivityTimer.current.cancel();
      inactivityRequestId.current = null;
    },
    [],
  );

  const send = useCallback(async (pageUuid: string | null) => {
    const current = stateRef.current;
    const message = current.input;
    if (!message.trim() || current.busy) return;
    const history = boundedAssistantHistory(current.turns);
    const requestId = crypto.randomUUID();
    dispatch({ type: "start", message, requestId });

    const channel = new Channel<ChatEvent>();
    let acceptingEvents = true;
    let timedOut = false;
    const clearInactivityTimer = () => {
      if (inactivityRequestId.current !== requestId) return;
      inactivityTimer.current.cancel();
      inactivityRequestId.current = null;
    };
    const resetInactivityTimer = () => {
      inactivityRequestId.current = requestId;
      inactivityTimer.current.schedule(() => {
        if (!acceptingEvents || inactivityRequestId.current !== requestId) return;
        inactivityRequestId.current = null;
        acceptingEvents = false;
        timedOut = true;
        dispatch({
          type: "fail",
          message: "AI stream timed out after 60 seconds without activity.",
        });
        dispatch({ type: "finish", requestId });
        void cancelChat(requestId);
      }, CHAT_INACTIVITY_TIMEOUT_MS);
    };
    channel.onmessage = (event) => {
      if (!acceptingEvents) return;
      resetInactivityTimer();
      dispatch({ type: "event", event });
    };
    resetInactivityTimer();
    try {
      await chatStream(history, message, current.allowWrites, pageUuid, requestId, channel);
    } catch (error) {
      if (!timedOut) dispatch({ type: "fail", message: String(error) });
    } finally {
      acceptingEvents = false;
      clearInactivityTimer();
      dispatch({ type: "finish", requestId });
    }
  }, []);

  const toggleAllowWrites = useCallback(async () => {
    const current = stateRef.current;
    if (current.busy) return;
    if (current.allowWrites) {
      dispatch({ type: "set_allow_writes", allow: false });
      return;
    }
    if (
      await confirm({
        title: "Allow AI writes?",
        description:
          "For the next request, the assistant may create notes and links. Every change is recorded in Undo history.",
        confirmLabel: "Allow once",
      })
    ) {
      dispatch({ type: "set_allow_writes", allow: true });
    }
  }, [confirm]);

  return {
    state,
    setInput: (input: string) => dispatch({ type: "set_input", input }),
    send,
    toggleAllowWrites,
    clear: () => {
      dispatch({ type: "clear" });
      if (!stateRef.current.busy) localStorage.removeItem(CHAT_STORAGE_KEY);
    },
    cancel: () => {
      const requestId = stateRef.current.activeRequestId;
      if (requestId) {
        if (inactivityRequestId.current === requestId) {
          inactivityTimer.current.cancel();
          inactivityRequestId.current = null;
        }
        void cancelChat(requestId);
      }
    },
  } as const;
}

export type AssistantController = ReturnType<typeof useAssistantController>;
