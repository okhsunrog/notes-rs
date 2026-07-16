import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { search, type Mode, type Node, type SearchHit } from "@/lib/api";

type Props = {
  hits: SearchHit[];
  setHits: React.Dispatch<React.SetStateAction<SearchHit[]>>;
  onOpenNode: (n: Node) => void | Promise<void>;
  onStatus: (s: string) => void;
};

export function SearchCard({ hits, setHits, onOpenNode, onStatus }: Props) {
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<Mode>("agentic");
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!query.trim()) return;
    setBusy(true);
    try {
      const out = await search(mode, query, 20);
      setHits(out);
      onStatus(`${out.length} hits (${mode})`);
    } catch (err) {
      onStatus(`error: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Search</CardTitle>
        <CardDescription>
          Lexical and semantic retrieval with optional provider reranking.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        <form onSubmit={submit} className="flex flex-col gap-2 sm:flex-row">
          <Input
            placeholder="query"
            value={query}
            onChange={(e) => setQuery(e.currentTarget.value)}
            className="flex-1"
          />
          <Select value={mode} onValueChange={(v) => setMode(v as Mode)}>
            <SelectTrigger className="w-full sm:w-48">
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
                onClick={() => void onOpenNode(h.node)}
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
  );
}
