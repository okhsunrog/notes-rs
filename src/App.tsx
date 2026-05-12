import { useState, useEffect, useCallback, useRef } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { Send, Wrench, CheckCircle2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { NoteEditor, type NoteEditorHandle } from "@/components/note-editor";

type Node = {
  id: number;
  uuid: string;
  kind: string;
  title: string | null;
  content: string;
  content_json: string | null;
  created_at: number;
  updated_at: number;
};

type SearchHit = { node: Node; score: number };
type Mode = "fts" | "vec" | "hybrid" | "agentic";

type ChatEvent =
  | { kind: "text_delta"; text: string }
  | { kind: "reasoning"; text: string }
  | { kind: "tool_start"; id: string; name: string; args: unknown }
  | { kind: "tool_end"; id: string; result: string }
  | { kind: "done"; text: string }
  | { kind: "error"; message: string };

type ToolCallView = { id: string; name: string; args: unknown; result?: string };
type ChatTurn = {
  role: "user" | "assistant";
  text: string;
  tools?: ToolCallView[];
};

function App() {
  const [title, setTitle] = useState("");
  const editorRef = useRef<NoteEditorHandle>(null);
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<Mode>("agentic");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState(false);
  const [chatInput, setChatInput] = useState("");
  const [chatLog, setChatLog] = useState<ChatTurn[]>([]);
  const [chatBusy, setChatBusy] = useState(false);
  const [entities, setEntities] = useState<Node[]>([]);
  const [editing, setEditing] = useState<Node | null>(null);
  const [editingTitle, setEditingTitle] = useState("");
  const editEditorRef = useRef<NoteEditorHandle>(null);

  const refreshEntities = useCallback(async () => {
    try {
      const list = await invoke<Node[]>("list_entities", { limit: 30 });
      setEntities(list);
    } catch {
      /* ignore */
    }
  }, []);

  useEffect(() => {
    refreshEntities();
    const t = setInterval(refreshEntities, 5000);
    return () => clearInterval(t);
  }, [refreshEntities]);

  async function createNote(e: React.FormEvent) {
    e.preventDefault();
    if (!editorRef.current) return;
    const { markdown, json } = await editorRef.current.serialize();
    const plain = markdown.trim();
    if (!plain) return;
    setBusy(true);
    try {
      const n = await invoke<Node>("create_node", {
        kind: "block",
        title: title.trim() || null,
        content: plain,
        contentJson: json,
      });
      setStatus(`created node #${n.id}`);
      setTitle("");
      editorRef.current.reset();
    } catch (err) {
      setStatus(`error: ${err}`);
    } finally {
      setBusy(false);
    }
  }

  async function search(e: React.FormEvent) {
    e.preventDefault();
    if (!query.trim()) return;
    setBusy(true);
    try {
      const cmd =
        mode === "fts"
          ? "search_fts"
          : mode === "vec"
            ? "search_vec"
            : mode === "hybrid"
              ? "search_hybrid"
              : "search_agentic";
      const out = await invoke<SearchHit[]>(cmd, { query, limit: 20 });
      setHits(out);
      setStatus(`${out.length} hits (${mode})`);
    } catch (err) {
      setStatus(`error: ${err}`);
    } finally {
      setBusy(false);
    }
  }

  function openEdit(node: Node) {
    setEditing(node);
    setEditingTitle(node.title ?? "");
  }

  async function saveEdit() {
    if (!editing || !editEditorRef.current) return;
    const { markdown, json } = await editEditorRef.current.serialize();
    setBusy(true);
    try {
      await invoke("update_node", {
        id: editing.id,
        title: editingTitle.trim() || null,
        content: markdown.trim(),
        contentJson: json,
      });
      setStatus(`updated node #${editing.id}`);
      setEditing(null);
      // Refresh the visible results in place
      setHits((hs) =>
        hs.map((h) =>
          h.node.id === editing.id
            ? {
                ...h,
                node: {
                  ...h.node,
                  title: editingTitle.trim() || null,
                  content: markdown.trim(),
                  content_json: json,
                },
              }
            : h,
        ),
      );
    } catch (err) {
      setStatus(`error: ${err}`);
    } finally {
      setBusy(false);
    }
  }

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
        next[next.length - 1] = { role: "assistant", text: `error: ${err}` };
        return next;
      });
    } finally {
      setChatBusy(false);
    }
  }

  return (
    <div className="min-h-screen bg-background text-foreground p-6">
      <div className="mx-auto max-w-4xl space-y-6">
        <header className="flex items-baseline justify-between">
          <h1 className="text-2xl font-semibold tracking-tight">notes-rs</h1>
          {status && <span className="text-xs text-muted-foreground">{status}</span>}
        </header>

        <Card>
          <CardHeader>
            <CardTitle>Create</CardTitle>
            <CardDescription>Add a block to the graph.</CardDescription>
          </CardHeader>
          <CardContent>
            <form onSubmit={createNote} className="space-y-3">
              <Input
                placeholder="title (optional)"
                value={title}
                onChange={(e) => setTitle(e.currentTarget.value)}
              />
              <NoteEditor ref={editorRef} placeholder="write a note…" />
              <Button type="submit" disabled={busy}>
                Create
              </Button>
            </form>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Search</CardTitle>
            <CardDescription>Hybrid retrieval with optional BGE reranker.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <form onSubmit={search} className="flex gap-2">
              <Input
                placeholder="query"
                value={query}
                onChange={(e) => setQuery(e.currentTarget.value)}
                className="flex-1"
              />
              <Select value={mode} onValueChange={(v) => setMode(v as Mode)}>
                <SelectTrigger className="w-48">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="agentic">agentic (rerank)</SelectItem>
                  <SelectItem value="hybrid">hybrid (RRF)</SelectItem>
                  <SelectItem value="fts">fts (BM25)</SelectItem>
                  <SelectItem value="vec">vec (semantic)</SelectItem>
                </SelectContent>
              </Select>
              <Button type="submit" disabled={busy}>
                Search
              </Button>
            </form>

            {hits.length > 0 && (
              <div className="space-y-2">
                {hits.map((h) => (
                  <button
                    key={h.node.id}
                    type="button"
                    onClick={() => openEdit(h.node)}
                    className="w-full rounded-md border bg-card p-3 text-left text-sm transition hover:bg-accent"
                  >
                    <div className="flex items-center gap-2">
                      <Badge variant="secondary">#{h.node.id}</Badge>
                      {h.node.title && <span className="font-medium">{h.node.title}</span>}
                      <span className="ml-auto font-mono text-xs text-muted-foreground">
                        {h.score.toFixed(4)}
                      </span>
                    </div>
                    <p className="mt-1 line-clamp-3 text-muted-foreground whitespace-pre-wrap">
                      {h.node.content}
                    </p>
                  </button>
                ))}
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Entities</CardTitle>
            <CardDescription>
              Extracted from your notes by the background worker. Top by mention count.
            </CardDescription>
          </CardHeader>
          <CardContent>
            {entities.length === 0 ? (
              <p className="text-sm italic text-muted-foreground">
                no entities yet — create a few notes and wait a moment.
              </p>
            ) : (
              <div className="flex flex-wrap gap-2">
                {entities.map((e) => (
                  <Badge key={e.id} variant="secondary" title={e.content}>
                    {e.title}
                  </Badge>
                ))}
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Chat</CardTitle>
            <CardDescription>Agentic RAG over your notes. Tool calls stream live.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <ScrollArea className="h-96 rounded-md border bg-card p-3">
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
                              <span className="text-muted-foreground">
                                {JSON.stringify(tool.args)}
                              </span>
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
              <Button type="submit" disabled={chatBusy}>
                <Send className="size-4" />
              </Button>
            </form>
          </CardContent>
        </Card>
      </div>

      <Dialog open={!!editing} onOpenChange={(v) => !v && setEditing(null)}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>Edit note</DialogTitle>
            <DialogDescription>
              {editing && (
                <>
                  Node #{editing.id} · {editing.kind}
                </>
              )}
            </DialogDescription>
          </DialogHeader>
          {editing && (
            <div className="space-y-3">
              <Input
                placeholder="title (optional)"
                value={editingTitle}
                onChange={(e) => setEditingTitle(e.currentTarget.value)}
              />
              <NoteEditor
                key={editing.id}
                ref={editEditorRef}
                initialJson={editing.content_json}
                initialPlain={editing.content}
                placeholder="content…"
              />
            </div>
          )}
          <DialogFooter>
            <Button
              variant="outline"
              type="button"
              onClick={() => setEditing(null)}
              disabled={busy}
            >
              Cancel
            </Button>
            <Button type="button" onClick={saveEdit} disabled={busy}>
              Save
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

export default App;
