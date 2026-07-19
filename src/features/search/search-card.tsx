import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { CalendarDays, FilePlus2, FileText, Loader2, Search, TextQuote } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { pageDisplayTitle, todayJournalDate } from "@/features/journal/journal-date";
import { useWorkspaceController } from "@/features/workspace/workspace-controller";
import {
  dispositionFromShiftKey,
  type OpenDisposition,
} from "@/features/workspace/workspace-model";
import {
  contentText,
  contentUuid,
  getPage,
  listJournals,
  listPages,
  loadSettings,
  search,
  type Content,
  type JournalDate,
  type Page,
  type SearchHit,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import {
  hasExactPageTitle,
  journalDateFromSearchQuery,
  movePaletteSelection,
  preservePaletteSelection,
  stablePaletteItems,
} from "./search-palette";
import {
  presentSearchResults,
  type SearchPresentation,
  type ServerSearchState,
} from "./search-presentation";

const LOCAL_DEBOUNCE_MS = 150;
const SERVER_DEBOUNCE_MS = 400;
const SERVER_TIMEOUT_MS = 10_000;
const RECENT_PAGE_LIMIT = 6;
const RECENT_JOURNAL_LIMIT = 4;

type Props = {
  variant?: "card" | "inline" | "dialog";
  onOpenContent: (content: Content, disposition?: OpenDisposition) => void | Promise<void>;
  onDismiss?: () => void;
};

type ContentRow = {
  kind: "content";
  key: string;
  hit: SearchHit;
  source: string;
};

type JournalRow = {
  kind: "journal";
  key: string;
  date: JournalDate;
  label: string;
};

type CreateNoteRow = {
  kind: "create_note";
  key: string;
  query: string;
  label: string;
};

type PaletteRow = ContentRow | JournalRow | CreateNoteRow;

export function SearchCard({ variant = "card", onOpenContent, onDismiss }: Props) {
  const controller = useWorkspaceController();
  const [query, setQuery] = useState("");
  const [localHits, setLocalHits] = useState<SearchHit[]>([]);
  const [serverHits, setServerHits] = useState<SearchHit[]>([]);
  const [serverState, setServerState] = useState<ServerSearchState>("absent");
  const [localPending, setLocalPending] = useState(false);
  const [resultsFrozen, setResultsFrozen] = useState(false);
  const [frozenPresentation, setFrozenPresentation] = useState<SearchPresentation>({
    primary: [],
    localExtras: [],
  });
  const [frozenSearchSettled, setFrozenSearchSettled] = useState(false);
  const [frozenPrimarySource, setFrozenPrimarySource] = useState("local FTS");
  const [frozenRecentPages, setFrozenRecentPages] = useState<Page[]>([]);
  const [frozenRecentJournals, setFrozenRecentJournals] = useState<Page[]>([]);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const localTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const serverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const localEpoch = useRef(0);
  const serverEpoch = useRef(0);
  const settingsQuery = useQuery({ queryKey: queryKeys.settings, queryFn: loadSettings });
  const pagesQuery = useQuery({
    queryKey: queryKeys.pages,
    queryFn: () => listPages({ filter: "notes" }),
  });
  const journalsQuery = useQuery({
    queryKey: queryKeys.journals,
    queryFn: () => listJournals({ limit: 7 }),
  });
  const aiSearchEnabled = settingsQuery.data?.aiSearchEnabled ?? true;
  const aiSearchAsYouType = settingsQuery.data?.aiSearchTrigger !== "enter_only";
  const aiSearchRerank = settingsQuery.data?.aiSearchRerank ?? true;
  const showSearchSources = settingsQuery.data?.searchDebugSources ?? false;
  const serverConfigured = Boolean(
    aiSearchEnabled &&
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

  const runServer = useCallback(
    async (value: string, epoch: number) => {
      try {
        const result = await withTimeout(
          search("semantic", value, 20, aiSearchRerank),
          SERVER_TIMEOUT_MS,
        );
        if (epoch !== serverEpoch.current) return;
        setServerHits(result);
        setServerState("success");
      } catch {
        if (epoch !== serverEpoch.current) return;
        setServerHits([]);
        setServerState("failed");
      }
    },
    [aiSearchRerank],
  );

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

    if (serverConfigured && aiSearchAsYouType && Array.from(value).length >= 3) {
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
  }, [aiSearchAsYouType, query, runLocal, runServer, serverConfigured]);

  const presented = useMemo(
    () => presentSearchResults({ localHits, serverHits, serverState }),
    [localHits, serverHits, serverState],
  );
  const displayedPresentation = useMemo<SearchPresentation>(
    () => ({
      primary: [
        ...stablePaletteItems(frozenPresentation.primary, presented.primary, resultsFrozen),
      ],
      localExtras: [
        ...stablePaletteItems(frozenPresentation.localExtras, presented.localExtras, resultsFrozen),
      ],
    }),
    [frozenPresentation, presented, resultsFrozen],
  );
  const displayedHits = useMemo(
    () => [...displayedPresentation.primary, ...displayedPresentation.localExtras],
    [displayedPresentation],
  );

  const queryValue = query.trim();
  const activeQuery = useRef(queryValue);
  activeQuery.current = queryValue;
  const observedPages = useRef(pagesQuery.data);
  useEffect(() => {
    if (observedPages.current === pagesQuery.data) return;
    observedPages.current = pagesQuery.data;
    const value = activeQuery.current;
    if (!value) return;

    clearSearchTimer(localTimer);
    const nextLocalEpoch = ++localEpoch.current;
    setResultsFrozen(false);
    setLocalPending(true);
    void runLocal(value, nextLocalEpoch);
  }, [pagesQuery.data, runLocal]);

  const searchSettled = !localPending && serverState !== "pending";
  const displayedSearchSettled = resultsFrozen ? frozenSearchSettled : searchSettled;
  const exactTitleMatch = hasExactPageTitle(queryValue, displayedHits);
  const journalDate = journalDateFromSearchQuery(queryValue);
  const primarySource =
    serverState === "success" && serverHits.length > 0 ? "server AI" : "local FTS";
  const displayedPrimarySource = resultsFrozen ? frozenPrimarySource : primarySource;
  const recentPages = (pagesQuery.data ?? []).slice(0, RECENT_PAGE_LIMIT);
  const recentJournals = (journalsQuery.data ?? [])
    .filter((page) => page.kind.kind !== "journal" || page.kind.date !== todayJournalDate())
    .slice(0, RECENT_JOURNAL_LIMIT);
  const displayedRecentPages = resultsFrozen ? frozenRecentPages : recentPages;
  const displayedRecentJournals = resultsFrozen ? frozenRecentJournals : recentJournals;

  const primaryRows = useMemo<ContentRow[]>(() => {
    if (!queryValue) {
      return [...displayedRecentPages, ...displayedRecentJournals].map((page) =>
        contentRow(pageHit(page), "recent"),
      );
    }
    return displayedPresentation.primary.map((hit) => contentRow(hit, displayedPrimarySource));
  }, [
    displayedPresentation.primary,
    displayedPrimarySource,
    displayedRecentJournals,
    displayedRecentPages,
    queryValue,
  ]);
  const localRows = useMemo<ContentRow[]>(
    () => displayedPresentation.localExtras.map((hit) => contentRow(hit, "local FTS")),
    [displayedPresentation.localExtras],
  );
  const leadingRows = useMemo<PaletteRow[]>(() => {
    if (!queryValue) {
      const today = todayJournalDate();
      return [{ kind: "journal", key: `journal:${today}`, date: today, label: "Open today" }];
    }
    const rows: PaletteRow[] = [];
    if (journalDate) {
      rows.push({
        kind: "journal",
        key: `journal:${journalDate}`,
        date: journalDate,
        label: `Open journal ${journalDate}`,
      });
    }
    if (displayedSearchSettled && exactTitleMatch) {
      rows.push({
        kind: "create_note",
        key: `note:${queryValue}`,
        query: queryValue,
        label: `Open “${queryValue}”`,
      });
    }
    return rows;
  }, [displayedSearchSettled, exactTitleMatch, journalDate, queryValue]);
  const trailingRows = useMemo<PaletteRow[]>(() => {
    if (!queryValue || !displayedSearchSettled || exactTitleMatch || displayedHits.length > 0) {
      return [];
    }
    return [
      {
        kind: "create_note",
        key: `note:${queryValue}`,
        query: queryValue,
        label: `Create “${queryValue}”`,
      },
    ];
  }, [displayedHits.length, displayedSearchSettled, exactTitleMatch, queryValue]);
  const rows = useMemo(
    () => [...leadingRows, ...primaryRows, ...localRows, ...trailingRows],
    [leadingRows, localRows, primaryRows, trailingRows],
  );
  const rowKeys = rows.map((row) => row.key).join("\u0000");

  useEffect(() => {
    setSelectedKey((current) => preservePaletteSelection(current, rows));
  }, [rowKeys]);

  const blockPageUuids = useMemo(
    () =>
      [
        ...new Set(
          [...primaryRows, ...localRows]
            .filter((row) => row.hit.content.kind === "block")
            .map((row) =>
              row.hit.content.kind === "block" ? row.hit.content.record.pageUuid : "",
            ),
        ),
      ].filter(Boolean),
    [localRows, primaryRows],
  );
  const blockPageQueries = useQueries({
    queries: blockPageUuids.map((pageUuid) => ({
      queryKey: queryKeys.page(pageUuid),
      queryFn: () => getPage(pageUuid),
    })),
  });
  const parentPages = new Map(
    blockPageUuids.map((uuid, index) => [uuid, blockPageQueries[index]?.data ?? null]),
  );

  const activateRow = useCallback(
    async (row: PaletteRow, shiftKey: boolean) => {
      const disposition = dispositionFromShiftKey(shiftKey);
      if (row.kind === "content") {
        await onOpenContent(row.hit.content, disposition);
      } else if (row.kind === "journal") {
        await controller.openJournal(row.date, disposition);
      } else {
        await controller.createNewNote(row.query, disposition);
      }
      onDismiss?.();
    },
    [controller, onDismiss, onOpenContent],
  );

  const triggerEnterOnlySearch = () => {
    if (
      aiSearchAsYouType ||
      !serverConfigured ||
      Array.from(queryValue).length < 3 ||
      serverState !== "idle"
    ) {
      return false;
    }
    clearSearchTimer(serverTimer);
    const nextServerEpoch = ++serverEpoch.current;
    setResultsFrozen(false);
    setServerHits([]);
    setServerState("pending");
    void runServer(queryValue, nextServerEpoch);
    return true;
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!resultsFrozen) {
        setFrozenPresentation(presented);
        setFrozenSearchSettled(searchSettled);
        setFrozenPrimarySource(primarySource);
        setFrozenRecentPages(recentPages);
        setFrozenRecentJournals(recentJournals);
      }
      setResultsFrozen(true);
      setSelectedKey((current) =>
        movePaletteSelection(current, rows, event.key === "ArrowDown" ? 1 : -1),
      );
      return;
    }
    if (event.key === "Enter") {
      if (triggerEnterOnlySearch()) {
        event.preventDefault();
        return;
      }
      const selected = rows.find((row) => row.key === selectedKey);
      if (!selected) return;
      event.preventDefault();
      void activateRow(selected, event.shiftKey);
    }
  };

  const handleQueryChange = (value: string) => {
    setResultsFrozen(false);
    setSelectedKey(null);
    setQuery(value);
  };

  const selectedIndex = rows.findIndex((row) => row.key === selectedKey);
  const input = (
    <div
      className={cn(
        "relative flex items-center",
        variant === "dialog" && "border-b border-border/60 px-4",
      )}
    >
      <Search className="pointer-events-none absolute left-3 size-4 text-muted-foreground" />
      <Input
        autoFocus={variant === "dialog"}
        role="combobox"
        aria-autocomplete="list"
        aria-controls="search-palette-results"
        aria-expanded={rows.length > 0}
        aria-activedescendant={selectedIndex >= 0 ? `search-option-${selectedIndex}` : undefined}
        placeholder="Search your knowledge…"
        value={query}
        onChange={(event) => handleQueryChange(event.currentTarget.value)}
        onKeyDown={handleKeyDown}
        className={cn(
          "pr-16 pl-9",
          variant === "dialog" &&
            "h-14 rounded-none border-0 bg-transparent px-8 text-base shadow-none focus-visible:ring-0 dark:bg-transparent",
        )}
      />
      {serverState === "pending" && (
        <span className="absolute right-3 flex items-center gap-1 text-xs text-muted-foreground">
          <Loader2 className="size-3 animate-spin" />
          AI…
        </span>
      )}
    </div>
  );

  const resultList = (
    <div
      id="search-palette-results"
      role="listbox"
      className={cn(
        "space-y-1",
        variant === "dialog" && "max-h-[min(60vh,32rem)] overflow-y-auto p-2",
      )}
    >
      {!queryValue && <SectionLabel>Recent</SectionLabel>}
      {leadingRows.map((row) => (
        <PaletteResultRow
          key={row.key}
          row={row}
          optionId={`search-option-${rows.indexOf(row)}`}
          selected={row.key === selectedKey}
          parentPage={null}
          showSource={showSearchSources}
          onActivate={activateRow}
          onSelect={setSelectedKey}
        />
      ))}
      {primaryRows.map((row) => (
        <PaletteResultRow
          key={row.key}
          row={row}
          optionId={`search-option-${rows.indexOf(row)}`}
          selected={row.key === selectedKey}
          parentPage={parentPageForRow(row, parentPages)}
          showSource={showSearchSources}
          onActivate={activateRow}
          onSelect={setSelectedKey}
        />
      ))}
      {localRows.length > 0 && <SectionLabel>Found locally</SectionLabel>}
      {localRows.map((row) => (
        <PaletteResultRow
          key={row.key}
          row={row}
          optionId={`search-option-${rows.indexOf(row)}`}
          selected={row.key === selectedKey}
          parentPage={parentPageForRow(row, parentPages)}
          showSource={showSearchSources}
          onActivate={activateRow}
          onSelect={setSelectedKey}
        />
      ))}
      {trailingRows.map((row) => (
        <PaletteResultRow
          key={row.key}
          row={row}
          optionId={`search-option-${rows.indexOf(row)}`}
          selected={row.key === selectedKey}
          parentPage={null}
          showSource={showSearchSources}
          onActivate={activateRow}
          onSelect={setSelectedKey}
        />
      ))}
      {queryValue && localPending && rows.length === 0 && (
        <p aria-live="polite" className="px-3 py-6 text-center text-sm text-muted-foreground">
          Searching on this device…
        </p>
      )}
    </div>
  );

  if (variant === "dialog") {
    return (
      <div className="overflow-hidden rounded-2xl">
        {input}
        {resultList}
        <div className="flex items-center justify-between border-t border-border/60 px-4 py-2 text-[11px] text-muted-foreground">
          <span>{displayedHits.length} results</span>
          <span>↑↓ · ⏎ open · ⇧⏎ open beside · esc</span>
        </div>
      </div>
    );
  }

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
        {input}
        {resultList}
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
  return <Card>{content}</Card>;
}

function contentRow(hit: SearchHit, source: string): ContentRow {
  return { kind: "content", key: `content:${contentUuid(hit.content)}`, hit, source };
}

function pageHit(page: Page): SearchHit {
  return { content: { kind: "page", record: page }, score: 0, snippet: null };
}

function parentPageForRow(row: ContentRow, pages: Map<string, Page | null>): Page | null {
  return row.hit.content.kind === "block"
    ? (pages.get(row.hit.content.record.pageUuid) ?? null)
    : null;
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <p className="px-3 pt-2 pb-1 text-[10px] font-semibold tracking-[0.12em] text-muted-foreground uppercase">
      {children}
    </p>
  );
}

function PaletteResultRow({
  row,
  optionId,
  selected,
  parentPage,
  showSource,
  onActivate,
  onSelect,
}: {
  row: PaletteRow;
  optionId: string;
  selected: boolean;
  parentPage: Page | null;
  showSource: boolean;
  onActivate: (row: PaletteRow, shiftKey: boolean) => void | Promise<void>;
  onSelect: (key: string) => void;
}) {
  return (
    <button
      id={optionId}
      role="option"
      aria-selected={selected}
      type="button"
      onMouseMove={() => onSelect(row.key)}
      onClick={(event) => void onActivate(row, event.shiftKey)}
      className={cn(
        "flex w-full items-start gap-3 rounded-xl px-3 py-2.5 text-left text-sm transition",
        selected ? "bg-accent text-accent-foreground" : "hover:bg-accent/60",
      )}
    >
      <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
        <RowIcon row={row} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2">
          <span className="truncate font-medium">{rowTitle(row)}</span>
          {row.kind === "content" && showSource && (
            <span className="ml-auto shrink-0 text-[10px] text-muted-foreground">{row.source}</span>
          )}
        </span>
        {row.kind === "content" && row.hit.content.kind === "block" && parentPage && (
          <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">
            {pageDisplayTitle(parentPage)} · block
          </span>
        )}
        {row.kind === "content" && <ResultSnippet hit={row.hit} />}
      </span>
    </button>
  );
}

function RowIcon({ row }: { row: PaletteRow }) {
  if (row.kind === "journal") return <CalendarDays className="size-4" />;
  if (row.kind === "create_note") return <FilePlus2 className="size-4" />;
  if (row.hit.content.kind === "block") return <TextQuote className="size-4" />;
  return row.hit.content.record.kind.kind === "journal" ? (
    <CalendarDays className="size-4" />
  ) : (
    <FileText className="size-4" />
  );
}

function rowTitle(row: PaletteRow): string {
  if (row.kind !== "content") return row.label;
  if (row.hit.content.kind === "page") return pageDisplayTitle(row.hit.content.record);
  return firstContentLine(row.hit.content.record.markdown) || "Block";
}

function firstContentLine(markdown: string): string {
  return (
    markdown
      .split("\n")
      .map((line) => line.trim())
      .find(Boolean) ?? ""
  );
}

function ResultSnippet({ hit }: { hit: SearchHit }) {
  const value = hit.snippet ?? (hit.content.kind === "block" ? contentText(hit.content) : "");
  if (!value) return null;
  let marked = false;
  return (
    <span className="mt-1 line-clamp-2 block text-xs leading-relaxed text-muted-foreground whitespace-pre-wrap">
      {value.split(/(<mark>|<\/mark>)/g).map((part, index) => {
        if (part === "<mark>") {
          marked = true;
          return null;
        }
        if (part === "</mark>") {
          marked = false;
          return null;
        }
        return marked ? (
          <mark key={`${index}-${part}`} className="rounded-sm bg-primary/15 text-foreground">
            {part}
          </mark>
        ) : (
          <span key={`${index}-${part}`}>{part}</span>
        );
      })}
    </span>
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
