import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  contentText,
  contentUuid,
  loadSettings,
  search,
  type Content,
  type SearchHit,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";
import { presentSearchResults, type ServerSearchState } from "./search-presentation";

const LOCAL_DEBOUNCE_MS = 150;
const SERVER_DEBOUNCE_MS = 400;
const SERVER_TIMEOUT_MS = 10_000;

type Props = {
  variant?: "card" | "inline" | "dialog";
  hits: SearchHit[];
  setHits: React.Dispatch<React.SetStateAction<SearchHit[]>>;
  onOpenContent: (content: Content, disposition?: OpenDisposition) => void | Promise<void>;
};

export function SearchCard({ variant = "card", hits, setHits, onOpenContent }: Props) {
  const [query, setQuery] = useState("");
  const [localHits, setLocalHits] = useState(hits);
  const [serverHits, setServerHits] = useState<SearchHit[]>([]);
  const [serverState, setServerState] = useState<ServerSearchState>("absent");
  const [localPending, setLocalPending] = useState(false);
  const localTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const serverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const localEpoch = useRef(0);
  const serverEpoch = useRef(0);
  const settingsQuery = useQuery({ queryKey: queryKeys.settings, queryFn: loadSettings });
  const serverConfigured = Boolean(
    settingsQuery.data?.syncServerUrl?.trim() &&
    settingsQuery.data.configuredKeys.includes("SYNC_TOKEN"),
  );

  const runLocal = useCallback(async (value: string, epoch: number) => {
    try {
      const result = await search("fts", value, 20);
      if (epoch !== localEpoch.current) return;
      setLocalHits(result);
    } catch {
      if (epoch !== localEpoch.current) return;
      setLocalHits([]);
    } finally {
      if (epoch === localEpoch.current) setLocalPending(false);
    }
  }, []);

  const runServer = useCallback(async (value: string, epoch: number) => {
    try {
      const result = await withTimeout(search("semantic", value, 20), SERVER_TIMEOUT_MS);
      if (epoch !== serverEpoch.current) return;
      setServerHits(result);
      setServerState("success");
    } catch {
      if (epoch !== serverEpoch.current) return;
      setServerHits([]);
      setServerState("failed");
    }
  }, []);

  useEffect(() => {
    const value = query.trim();
    const nextLocalEpoch = ++localEpoch.current;
    const nextServerEpoch = ++serverEpoch.current;
    clearSearchTimer(localTimer);
    clearSearchTimer(serverTimer);
    setServerHits([]);

    if (!value) {
      setLocalHits([]);
      setLocalPending(false);
      setServerState(serverConfigured ? "idle" : "absent");
      return;
    }

    setLocalHits([]);
    setLocalPending(true);
    localTimer.current = setTimeout(() => {
      localTimer.current = null;
      void runLocal(value, nextLocalEpoch);
    }, LOCAL_DEBOUNCE_MS);

    if (serverConfigured && Array.from(value).length >= 3) {
      setServerState("pending");
      serverTimer.current = setTimeout(() => {
        serverTimer.current = null;
        void runServer(value, nextServerEpoch);
      }, SERVER_DEBOUNCE_MS);
    } else {
      setServerState(serverConfigured ? "idle" : "absent");
    }

    return () => {
      clearSearchTimer(localTimer);
      clearSearchTimer(serverTimer);
      localEpoch.current += 1;
      serverEpoch.current += 1;
    };
  }, [query, runLocal, runServer, serverConfigured]);

  const presented = useMemo(
    () => presentSearchResults({ localHits, serverHits, serverState }),
    [localHits, serverHits, serverState],
  );

  useEffect(() => {
    setHits([...presented.primary, ...presented.localExtras]);
  }, [presented, setHits]);

  const submitImmediately = (event: React.FormEvent) => {
    event.preventDefault();
    const value = query.trim();
    if (!value) return;

    clearSearchTimer(localTimer);
    const nextLocalEpoch = ++localEpoch.current;
    setLocalPending(true);
    void runLocal(value, nextLocalEpoch);

    clearSearchTimer(serverTimer);
    const nextServerEpoch = ++serverEpoch.current;
    setServerHits([]);
    if (serverConfigured && Array.from(value).length >= 3) {
      setServerState("pending");
      void runServer(value, nextServerEpoch);
    } else {
      setServerState(serverConfigured ? "idle" : "absent");
    }
  };

  const primarySource =
    serverState === "success" && serverHits.length > 0 ? "server AI" : "local FTS";
  const content = (
    <>
      <CardHeader>
        <CardTitle>{variant === "inline" ? "Find something you wrote" : "Search"}</CardTitle>
        <CardDescription>
          {variant === "inline"
            ? "Search across titles, blocks, and meaning."
            : "Local results arrive first; server AI refines them when configured."}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        <form onSubmit={submitImmediately} className="relative">
          <Input
            placeholder="Search your knowledge…"
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            className="pr-16"
          />
          {serverState === "pending" && (
            <span className="absolute top-1/2 right-3 flex -translate-y-1/2 items-center gap-1 text-xs text-muted-foreground">
              <Loader2 className="size-3 animate-spin" />
              AI…
            </span>
          )}
        </form>

        {query.trim() && (
          <p aria-live="polite" className="text-xs text-muted-foreground">
            {localPending && presented.primary.length === 0
              ? "Searching on this device…"
              : `${presented.primary.length} result${presented.primary.length === 1 ? "" : "s"}`}
          </p>
        )}

        <SearchResultList
          hits={presented.primary}
          source={primarySource}
          onOpenContent={onOpenContent}
        />
        {presented.localExtras.length > 0 && (
          <section className="space-y-2 border-t border-border/60 pt-3">
            <p className="text-xs font-medium text-muted-foreground">Found locally</p>
            <SearchResultList
              hits={presented.localExtras}
              source="local FTS"
              onOpenContent={onOpenContent}
            />
          </section>
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

function SearchResultList({
  hits,
  source,
  onOpenContent,
}: {
  hits: SearchHit[];
  source: string;
  onOpenContent: Props["onOpenContent"];
}) {
  if (hits.length === 0) return null;
  return (
    <div className="space-y-2">
      {hits.map((hit, index) => (
        <button
          key={contentUuid(hit.content)}
          type="button"
          onClick={(event) =>
            void onOpenContent(hit.content, dispositionFromShiftKey(event.shiftKey))
          }
          className="w-full rounded-md border bg-card p-3 text-left text-sm transition hover:bg-accent"
        >
          <div className="flex items-center gap-2">
            <Badge variant="secondary">{hit.content.kind}</Badge>
            {hit.content.kind === "page" && hit.content.record.title && (
              <span className="font-medium">{hit.content.record.title}</span>
            )}
            <span className="ml-auto text-xs text-muted-foreground">
              #{index + 1} · {source}
            </span>
          </div>
          <p className="mt-1 line-clamp-3 text-muted-foreground whitespace-pre-wrap">
            {contentText(hit.content)}
          </p>
        </button>
      ))}
    </div>
  );
}

function clearSearchTimer(timer: React.MutableRefObject<ReturnType<typeof setTimeout> | null>) {
  if (timer.current !== null) clearTimeout(timer.current);
  timer.current = null;
}

async function withTimeout<T>(promise: Promise<T>, timeoutMs: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<T>((_, reject) => {
        timer = setTimeout(() => reject(new Error("search timed out")), timeoutMs);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
