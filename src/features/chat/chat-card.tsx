import { useState } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { Send, Wrench, CheckCircle2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
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
    <Card className="flex h-full flex-col">
      <CardHeader>
        <CardTitle>Chat</CardTitle>
        <CardDescription>Agentic RAG over your notes. Tool calls stream live.</CardDescription>
      </CardHeader>
      <CardContent className="flex min-h-0 flex-1 flex-col space-y-3">
        <ScrollArea className="min-h-0 flex-1 rounded-md border bg-card p-3">
          {chatLog.length === 0 && (
            <p className="text-sm italic text-muted-foreground">
              Ask the agent something about your notes…
            </p>
          )}
          <div className="space-y-4">
            {chatLog.map((t, i) => (
              <div key={i}>
                <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  {t.role}
                </div>
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
                          <span className="text-muted-foreground">{JSON.stringify(tool.args)}</span>
                          )
                        </span>
                      </li>
                    ))}
                  </ul>
                )}
                <p className="whitespace-pre-wrap text-sm leading-relaxed">{t.text}</p>
                {i < chatLog.length - 1 && <Separator className="mt-3" />}
              </div>
            ))}
            {chatBusy && <p className="text-sm italic text-muted-foreground">thinking…</p>}
          </div>
        </ScrollArea>

        <form onSubmit={sendChat} className="flex gap-2">
          <Input
            placeholder="message"
            value={chatInput}
            onChange={(e) => setChatInput(e.currentTarget.value)}
            disabled={chatBusy}
            className="flex-1"
          />
          <Button type="submit" aria-label="Send message" disabled={chatBusy}>
            <Send className="size-4" />
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}
