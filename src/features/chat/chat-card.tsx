import { useState } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { ArrowUp, CheckCircle2, Sparkles, Wrench } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { ChatEvent, ChatTurn } from "@/lib/api";

export function ChatCard() {
  const [chatInput, setChatInput] = useState("");
  const [chatLog, setChatLog] = useState<ChatTurn[]>([]);
  const [chatBusy, setChatBusy] = useState(false);

  async function sendChat(e: React.FormEvent) {
    e.preventDefault();
    if (!chatInput.trim() || chatBusy) return;
    const message = chatInput;
    setChatInput("");
    const history = chatLog.map((t) => ({ role: t.role, text: t.text }));
    setChatLog((l) => [
      ...l,
      { role: "user", text: message },
      { role: "assistant", text: "", tools: [] },
    ]);
    setChatBusy(true);

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
          case "error":
            last.text = `error: ${ev.message}`;
            break;
        }
        next[next.length - 1] = last;
        return next;
      });
    };

    try {
      await invoke("chat_stream", { history, message, onEvent: channel });
    } catch (err) {
      setChatLog((l) => {
        const next = [...l];
        next[next.length - 1] = { role: "assistant", text: `error: ${String(err)}` };
        return next;
      });
    } finally {
      setChatBusy(false);
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
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
                "Summarize this note",
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
                      <span className="font-mono">
                        {tool.name}(
                        <span className="text-muted-foreground">{JSON.stringify(tool.args)}</span>)
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              <p className="whitespace-pre-wrap text-[13px] leading-relaxed">{t.text}</p>
            </div>
          ))}
          {chatBusy && <p className="text-sm italic text-muted-foreground">thinking…</p>}
        </div>
      </ScrollArea>

      <form
        onSubmit={sendChat}
        className="mt-2 flex gap-2 rounded-2xl border border-border/70 bg-card/75 p-1.5 shadow-sm focus-within:border-primary/30 focus-within:ring-3 focus-within:ring-primary/10"
      >
        <Input
          placeholder="Ask about your notes…"
          value={chatInput}
          onChange={(e) => setChatInput(e.currentTarget.value)}
          disabled={chatBusy}
          className="h-9 flex-1 border-0 bg-transparent shadow-none focus-visible:ring-0"
        />
        <Button
          type="submit"
          size="icon-sm"
          aria-label="Send message"
          disabled={chatBusy}
          className="size-9 rounded-xl"
        >
          <ArrowUp className="size-4" />
        </Button>
      </form>
    </div>
  );
}
