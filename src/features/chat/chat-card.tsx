import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { ArrowUp, CheckCircle2, PencilLine, Sparkles, Square, Trash2, Wrench } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cancelChat, chatStream, type ChatEvent, type ChatTurn, type Node } from "@/lib/api";

const CHAT_STORAGE_KEY = "notes-rs.chat.v1";
const MarkdownResponse = lazy(() => import("@/features/chat/markdown-response"));

function loadStoredChat(): ChatTurn[] {
  try {
    const value = JSON.parse(localStorage.getItem(CHAT_STORAGE_KEY) ?? "[]");
    return Array.isArray(value) ? value.slice(-50) : [];
  } catch {
    return [];
  }
}

function boundedHistory(turns: ChatTurn[]) {
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

export function ChatCard({ node }: { node: Node | null }) {
  const [chatInput, setChatInput] = useState("");
  const [chatLog, setChatLog] = useState<ChatTurn[]>(loadStoredChat);
  const [chatBusy, setChatBusy] = useState(false);
  const [allowWrites, setAllowWrites] = useState(false);
  const activeRequest = useRef<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const endRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const textarea = inputRef.current;
    if (!textarea) return;
    textarea.style.height = "0px";
    textarea.style.height = `${Math.min(Math.max(textarea.scrollHeight, 72), 240)}px`;
  }, [chatInput]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end", behavior: chatBusy ? "smooth" : "auto" });
  }, [chatLog, chatBusy]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      localStorage.setItem(CHAT_STORAGE_KEY, JSON.stringify(chatLog.slice(-50)));
    }, 200);
    return () => window.clearTimeout(timer);
  }, [chatLog]);

  async function sendChat(e: React.FormEvent) {
    e.preventDefault();
    if (!chatInput.trim() || chatBusy) return;
    const message = chatInput;
    setChatInput("");
    const history = boundedHistory(chatLog);
    setChatLog((l) => [
      ...l,
      { role: "user", text: message },
      { role: "assistant", text: "", tools: [] },
    ]);
    setChatBusy(true);
    const requestId = crypto.randomUUID();
    activeRequest.current = requestId;

    const channel = new Channel<ChatEvent>();
    channel.onmessage = (ev) => {
      setChatLog((l) => {
        const next = [...l];
        const last = { ...next[next.length - 1] };
        if (last.role !== "assistant") return l;
        switch (ev.kind) {
          case "text_delta":
            last.text += ev.text;
            break;
          case "tool_start":
            last.tools = [...(last.tools ?? []), { id: ev.id, name: ev.name, args: ev.args }];
            break;
          case "tool_end":
            last.tools = (last.tools ?? []).map((t) =>
              t.id === ev.id ? { ...t, result: ev.result } : t,
            );
            break;
          case "usage":
            last.usage = {
              inputTokens: ev.inputTokens,
              outputTokens: ev.outputTokens,
              totalTokens: ev.totalTokens,
            };
            break;
          case "error":
            last.text = `error: ${ev.message}`;
            break;
          case "cancelled":
            last.text = `${last.text}\n\n_Stopped._`;
            break;
        }
        next[next.length - 1] = last;
        return next;
      });
    };

    try {
      await chatStream(history, message, allowWrites, node?.id ?? null, requestId, channel);
    } catch (err) {
      setChatLog((l) => {
        const next = [...l];
        next[next.length - 1] = { role: "assistant", text: `error: ${String(err)}` };
        return next;
      });
    } finally {
      setChatBusy(false);
      setAllowWrites(false);
      activeRequest.current = null;
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="mb-1 flex items-center justify-between gap-2 px-1 text-[10px] text-muted-foreground">
        <span className="truncate">
          {node ? `Context: ${node.title || `node #${node.id}`}` : "No active note context"}
        </span>
        {chatLog.length > 0 && (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            disabled={chatBusy}
            aria-label="Clear conversation"
            onClick={() => {
              setChatLog([]);
              localStorage.removeItem(CHAT_STORAGE_KEY);
            }}
          >
            <Trash2 className="size-3.5" />
          </Button>
        )}
      </div>
      <ScrollArea className="min-h-0 flex-1 px-1">
        {chatLog.length === 0 && (
          <div className="flex min-h-[24rem] flex-col items-center justify-center px-4 text-center">
            <div className="mb-4 flex size-11 items-center justify-center rounded-2xl bg-primary/10 text-primary ring-1 ring-primary/15">
              <Sparkles className="size-5" />
            </div>
            <p className="text-sm font-semibold">Ask your knowledge base</p>
            <p className="mt-2 max-w-[16rem] text-xs leading-relaxed text-muted-foreground">
              I can find connections, summarize ideas, and trace information across your notes.
            </p>
            <div className="mt-5 flex w-full max-w-[17rem] flex-col gap-1.5">
              {[
                "What have I been thinking about?",
                "Find related ideas",
                ...(node ? ["Summarize this note"] : []),
              ].map((prompt) => (
                <button
                  key={prompt}
                  type="button"
                  onClick={() => setChatInput(prompt)}
                  className="rounded-xl border border-border/60 bg-card/55 px-3 py-2 text-left text-[11px] text-muted-foreground transition hover:border-primary/25 hover:bg-primary/5 hover:text-foreground"
                >
                  {prompt}
                </button>
              ))}
            </div>
          </div>
        )}
        <div className="space-y-5 py-3">
          {chatLog.map((t, i) => (
            <div
              key={i}
              className={
                t.role === "user"
                  ? "ml-8 rounded-2xl rounded-br-md bg-primary px-3.5 py-2.5 text-primary-foreground shadow-sm"
                  : "pr-2"
              }
            >
              {t.role === "assistant" && (
                <div className="mb-2 flex items-center gap-2 text-[10px] font-semibold tracking-wide text-primary uppercase">
                  <Sparkles className="size-3" /> notes assistant
                </div>
              )}
              {t.tools && t.tools.length > 0 && (
                <ul className="mb-2 space-y-1">
                  {t.tools.map((tool, j) => (
                    <li
                      key={j}
                      className="flex items-start gap-2 rounded-sm bg-muted px-2 py-1 text-xs"
                    >
                      {tool.result ? (
                        <CheckCircle2 className="mt-0.5 size-3 shrink-0 text-emerald-500" />
                      ) : (
                        <Wrench className="mt-0.5 size-3 shrink-0 animate-pulse" />
                      )}
                      <span className="min-w-0 break-all font-mono">
                        {tool.name}(
                        <span className="text-muted-foreground">{JSON.stringify(tool.args)}</span>)
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              {t.role === "assistant" ? (
                <Suspense
                  fallback={
                    <p className="whitespace-pre-wrap text-[13px] leading-relaxed">{t.text}</p>
                  }
                >
                  <MarkdownResponse>{t.text}</MarkdownResponse>
                </Suspense>
              ) : (
                <p className="whitespace-pre-wrap text-[13px] leading-relaxed">{t.text}</p>
              )}
              {t.role === "assistant" && t.usage && t.usage.totalTokens > 0 && (
                <p className="mt-2 text-[10px] text-muted-foreground">
                  {t.usage.inputTokens.toLocaleString()} in ·{" "}
                  {t.usage.outputTokens.toLocaleString()}
                  {" out · "}
                  {t.usage.totalTokens.toLocaleString()} tokens
                </p>
              )}
            </div>
          ))}
          {chatBusy && <p className="text-sm italic text-muted-foreground">thinking…</p>}
          <div ref={endRef} />
        </div>
      </ScrollArea>

      <form
        onSubmit={sendChat}
        className="mt-2 rounded-2xl border border-border/70 bg-card/85 p-2 shadow-sm focus-within:border-primary/30 focus-within:ring-3 focus-within:ring-primary/10"
      >
        <textarea
          ref={inputRef}
          rows={3}
          placeholder="Ask about your notes…"
          value={chatInput}
          onChange={(event) => setChatInput(event.currentTarget.value)}
          disabled={chatBusy}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              event.currentTarget.form?.requestSubmit();
            }
          }}
          className="block max-h-60 min-h-[4.5rem] w-full resize-none overflow-y-auto bg-transparent px-2 py-1.5 text-sm leading-relaxed outline-none placeholder:text-muted-foreground/70 disabled:opacity-60"
        />
        <div className="mt-1 flex items-center gap-2">
          <Button
            type="button"
            size="sm"
            variant={allowWrites ? "default" : "ghost"}
            aria-label="Allow AI writes for the next request"
            aria-pressed={allowWrites}
            disabled={chatBusy}
            title="Allow create/link tools for the next request only"
            onClick={() => {
              if (allowWrites) {
                setAllowWrites(false);
                return;
              }
              if (
                window.confirm(
                  "Allow the AI to create notes and links during the next request? The changes will be added to Undo history.",
                )
              )
                setAllowWrites(true);
            }}
            className="h-8 rounded-xl px-2.5"
          >
            <PencilLine className="size-3.5" />
            <span className="text-[10px]">{allowWrites ? "Writes allowed" : "Allow writes"}</span>
          </Button>
          <span className="text-[10px] text-muted-foreground">Shift Enter for a new line</span>
          <Button
            type={chatBusy ? "button" : "submit"}
            size="icon-sm"
            aria-label={chatBusy ? "Stop response" : "Send message"}
            disabled={!chatBusy && !chatInput.trim()}
            onClick={
              chatBusy
                ? () => {
                    if (activeRequest.current) void cancelChat(activeRequest.current);
                  }
                : undefined
            }
            className="ml-auto size-9 rounded-xl"
          >
            {chatBusy ? (
              <Square className="size-3.5 fill-current" />
            ) : (
              <ArrowUp className="size-4" />
            )}
          </Button>
        </div>
      </form>
    </div>
  );
}
