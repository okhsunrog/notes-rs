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
import {
  contentText,
  contentUuid,
  search,
  type Content,
  type SearchHit,
  type SearchMode,
} from "@/lib/api";

type Props = {
  variant?: "card" | "inline" | "dialog";
  hits: SearchHit[];
  setHits: React.Dispatch<React.SetStateAction<SearchHit[]>>;
  onOpenContent: (content: Content) => void | Promise<void>;
  onStatus: (s: string) => void;
};

export function SearchCard({ variant = "card", hits, setHits, onOpenContent, onStatus }: Props) {
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<SearchMode>("fts");
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

  const content = (
    <>
      <CardHeader>
        <CardTitle>{variant === "inline" ? "Find something you wrote" : "Search"}</CardTitle>
        <CardDescription>
          {variant === "inline"
            ? "Search across titles, blocks, and meaning."
            : "Lexical and semantic retrieval with optional provider reranking."}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        <form onSubmit={submit} className="flex flex-col gap-2 sm:flex-row">
          <Input
            placeholder="Search your knowledge…"
            value={query}
            onChange={(e) => setQuery(e.currentTarget.value)}
            className="flex-1"
          />
          <Select value={mode} onValueChange={(v) => setMode(v as SearchMode)}>
            <SelectTrigger className="w-full sm:w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="fts">On this device (FTS)</SelectItem>
              <SelectItem value="semantic">AI on server</SelectItem>
            </SelectContent>
          </Select>
          <Button type="submit" disabled={busy} className="rounded-xl">
            Search
          </Button>
        </form>

        {hits.length > 0 && (
          <div className="space-y-2">
            {hits.map((h, index) => (
              <button
                key={contentUuid(h.content)}
                type="button"
                onClick={() => void onOpenContent(h.content)}
                className="w-full rounded-md border bg-card p-3 text-left text-sm transition hover:bg-accent"
              >
                <div className="flex items-center gap-2">
                  <Badge variant="secondary">{h.content.kind}</Badge>
                  {h.content.kind === "page" && h.content.record.title && (
                    <span className="font-medium">{h.content.record.title}</span>
                  )}
                  <span className="ml-auto text-xs text-muted-foreground">
                    #{index + 1} · {mode === "semantic" ? "server AI" : "local FTS"}
                  </span>
                </div>
                <p className="mt-1 line-clamp-3 text-muted-foreground whitespace-pre-wrap">
                  {contentText(h.content)}
                </p>
              </button>
            ))}
          </div>
        )}
      </CardContent>
    </>
  );

  if (variant === "inline") {
    return (
      <Card className="gap-4 rounded-2xl border-border/60 bg-card/65 py-5 shadow-sm backdrop-blur [&_[data-slot=card-header]]:px-5 [&_[data-slot=card-content]]:px-5">
        {content}
      </Card>
    );
  }
  if (variant === "dialog") {
    return (
      <div className="py-2 [&_[data-slot=card-header]]:px-3 [&_[data-slot=card-content]]:px-3">
        {content}
      </div>
    );
  }
  return <Card>{content}</Card>;
}
