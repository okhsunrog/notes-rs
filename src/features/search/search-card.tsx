import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import {
  ArrowLeft,
  CalendarDays,
  FilePlus2,
  FileText,
  Loader2,
  Search,
  TextQuote,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useCompactLayout } from "@/app/use-compact-layout";
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
import { notifyError } from "@/lib/notify";
import { DebouncedAction } from "@/lib/debounced-action";
import { cn } from "@/lib/utils";
import {
  hasExactPageTitle,
  journalDateFromSearchQuery,
  movePaletteSelection,
  preservePaletteSelection,
  resolvePendingPaletteEnter,
} from "./search-palette";
import {
  presentSearchResults,
  type SearchPresentation,
  type ServerSearchState,
} from "./search-presentation";

const LOCAL_DEBOUNCE_MS = 150;
const SERVER_DEBOUNCE_MS = 400;
const RECENT_PAGE_LIMIT = 6;
const RECENT_JOURNAL_LIMIT = 4;

type Props = {
  variant?: "card" | "inline" | "dialog" | "dashboard" | "sidebar";
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

type FrozenSearchSnapshot = {
  presentation: SearchPresentation;
  searchSettled: boolean;
  recentPages: Page[];
  recentJournals: Page[];
};

export function SearchCard({ variant = "card", onOpenContent, onDismiss }: Props) {
  const compact = useCompactLayout();
  const controller = useWorkspaceController();
  const [query, setQuery] = useState("");
  const [localHits, setLocalHits] = useState<SearchHit[]>([]);
  const [serverHits, setServerHits] = useState<SearchHit[]>([]);
  const [serverState, setServerState] = useState<ServerSearchState>("absent");
  const [localPending, setLocalPending] = useState(false);
  const [localError, setLocalError] = useState(false);
  const [frozenSnapshot, setFrozenSnapshot] = useState<FrozenSearchSnapshot | null>(null);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [pendingEnter, setPendingEnter] = useState<{
    query: string;
    shiftKey: boolean;
  } | null>(null);
  const localDebounce = useRef(new DebouncedAction()).current;
  const serverDebounce = useRef(new DebouncedAction()).current;
  const localEpoch = useRef(0);
  const serverEpoch = useRef(0);
  const settingsQuery = useQuery({ queryKey: queryKeys.settings, queryFn: loadSettings });
  const pagesQuery = useQuery({
    queryKey: queryKeys.pageList("notes", 10_000),
    queryFn: () => listPages({ filter: "notes", limit: 10_000 }),
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

  const runLocal = useCallback(
    async (value: string, epoch: number, onSuccess?: (result: SearchHit[]) => void) => {
      try {
        const result = await search("fts", value, 20);
        if (epoch !== localEpoch.current) return;
        setLocalHits(result);
        setLocalError(false);
        onSuccess?.(result);
      } catch (error) {
        if (epoch !== localEpoch.current) return;
        setLocalHits([]);
        setLocalError(true);
        setPendingEnter(null);
        notifyError("search", error);
      } finally {
        if (epoch === localEpoch.current) setLocalPending(false);
      }
    },
    [],
  );

  const runServer = useCallback(async (value: string, epoch: number, rerank: boolean) => {
    try {
      // The shared Rust transport owns the request timeout. A shorter Promise timeout here cannot
      // cancel the IPC request and used to discard successful semantic responses that arrived late.
      const result = await search("semantic", value, 20, rerank);
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
    localDebounce.cancel();
    serverDebounce.cancel();
    setServerHits([]);

    if (!value) {
      setLocalHits([]);
      setLocalPending(false);
      setLocalError(false);
      setServerState(serverConfigured ? "idle" : "absent");
      return;
    }

    setLocalHits([]);
    setLocalPending(true);
    setLocalError(false);
    localDebounce.schedule(() => {
      void runLocal(value, nextLocalEpoch);
    }, LOCAL_DEBOUNCE_MS);

    if (serverConfigured && aiSearchAsYouType && Array.from(value).length >= 3) {
      setServerState("pending");
      serverDebounce.schedule(() => {
        void runServer(value, nextServerEpoch, false);
      }, SERVER_DEBOUNCE_MS);
    } else {
      setServerState(serverConfigured ? "idle" : "absent");
    }

    return () => {
      localDebounce.cancel();
      serverDebounce.cancel();
      localEpoch.current += 1;
      serverEpoch.current += 1;
    };
  }, [
    aiSearchAsYouType,
    localDebounce,
    query,
    runLocal,
    runServer,
    serverConfigured,
    serverDebounce,
  ]);

  const presented = useMemo(
    () => presentSearchResults({ localHits, serverHits, serverState }),
    [localHits, serverHits, serverState],
  );
  const displayedPresentation = frozenSnapshot?.presentation ?? presented;
  const displayedHits = useMemo(
    () => [...displayedPresentation.primary, ...displayedPresentation.localExtras],
    [displayedPresentation],
  );

  const queryValue = query.trim();
  const activeQuery = useRef(queryValue);
  activeQuery.current = queryValue;
  const observedPages = useRef(pagesQuery.data);
  // Page list invalidation is the clean title/delete refresh signal. Block events do not yet
  // expose a search revision query; coupling this to unrelated history state would be misleading.
  useEffect(() => {
    if (observedPages.current === pagesQuery.data) return;
    observedPages.current = pagesQuery.data;
    const value = activeQuery.current;
    if (!value) return;

    localDebounce.cancel();
    const nextLocalEpoch = ++localEpoch.current;
    setLocalPending(true);
    setLocalError(false);
    void runLocal(value, nextLocalEpoch, (result) => {
      const refreshed = presentSearchResults({ localHits: result, serverHits, serverState });
      setFrozenSnapshot((current) =>
        current === null
          ? null
          : {
              ...current,
              presentation: refreshed,
              searchSettled: serverState !== "pending",
              recentPages: (pagesQuery.data ?? []).slice(0, RECENT_PAGE_LIMIT),
            },
      );
    });
  }, [localDebounce, pagesQuery.data, runLocal, serverHits, serverState]);

  const searchSettled = !localPending && serverState !== "pending";
  const displayedSearchSettled = frozenSnapshot?.searchSettled ?? searchSettled;
  const exactTitleMatch = hasExactPageTitle(queryValue, displayedHits);
  const journalDate = journalDateFromSearchQuery(queryValue);
  const displayedPrimarySource = displayedPresentation.primarySource;
  const recentPages = (pagesQuery.data ?? []).slice(0, RECENT_PAGE_LIMIT);
  const recentJournals = (journalsQuery.data ?? [])
    .filter((page) => page.kind.kind !== "journal" || page.kind.date !== todayJournalDate())
    .slice(0, RECENT_JOURNAL_LIMIT);
  const displayedRecentPages = frozenSnapshot?.recentPages ?? recentPages;
  const displayedRecentJournals = frozenSnapshot?.recentJournals ?? recentJournals;

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
    if (displayedSearchSettled && !localError && exactTitleMatch) {
      rows.push({
        kind: "create_note",
        key: `note:${queryValue}`,
        query: queryValue,
        label: `Open “${queryValue}”`,
      });
    }
    return rows;
  }, [displayedSearchSettled, exactTitleMatch, journalDate, localError, queryValue]);
  const trailingRows = useMemo<PaletteRow[]>(() => {
    if (
      !queryValue ||
      !displayedSearchSettled ||
      localError ||
      exactTitleMatch ||
      displayedHits.length > 0
    ) {
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
  }, [displayedHits.length, displayedSearchSettled, exactTitleMatch, localError, queryValue]);
  const rows = useMemo(
    () => [...leadingRows, ...primaryRows, ...localRows, ...trailingRows],
    [leadingRows, localRows, primaryRows, trailingRows],
  );
  const rowKeys = rows.map((row) => row.key).join("\u0000");

  useEffect(() => {
    setSelectedKey((current) => preservePaletteSelection(current, rows));
  }, [rowKeys]);

  const blockPageUuids = useMemo(
    () => [
      ...new Set(
        [...primaryRows, ...localRows].flatMap((row) =>
          row.hit.content.kind === "block" ? [row.hit.content.record.pageUuid] : [],
        ),
      ),
    ],
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

  useEffect(() => {
    const activation = resolvePendingPaletteEnter(
      pendingEnter,
      queryValue,
      displayedSearchSettled,
      rows,
    );
    if (!activation) return;
    setPendingEnter(null);
    void activateRow(activation.item, activation.shiftKey);
  }, [activateRow, displayedSearchSettled, pendingEnter, queryValue, rows]);

  const triggerEnterOnlySearch = () => {
    if (
      aiSearchAsYouType ||
      !serverConfigured ||
      Array.from(queryValue).length < 3 ||
      (serverState !== "idle" && serverState !== "failed")
    ) {
      return false;
    }
    serverDebounce.cancel();
    const nextServerEpoch = ++serverEpoch.current;
    setFrozenSnapshot(null);
    setServerHits([]);
    setServerState("pending");
    void runServer(queryValue, nextServerEpoch, aiSearchRerank);
    return true;
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      setFrozenSnapshot(
        (current) =>
          current ?? { presentation: presented, searchSettled, recentPages, recentJournals },
      );
      setSelectedKey((current) =>
        movePaletteSelection(current, rows, event.key === "ArrowDown" ? 1 : -1),
      );
      return;
    }
    if (event.key === "Enter") {
      if (triggerEnterOnlySearch()) {
        event.preventDefault();
        setPendingEnter(null);
        return;
      }
      if (queryValue && !displayedSearchSettled) {
        event.preventDefault();
        setPendingEnter({ query: queryValue, shiftKey: event.shiftKey });
        return;
      }
      const selected = rows.find((row) => row.key === selectedKey);
      if (!selected) return;
      event.preventDefault();
      void activateRow(selected, event.shiftKey);
    }
  };

  const handleQueryChange = (value: string) => {
    setFrozenSnapshot(null);
    setSelectedKey(null);
    setPendingEnter(null);
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
      {variant === "dialog" && compact && onDismiss && (
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label="Close search"
          onClick={onDismiss}
          className="mr-1 shrink-0 rounded-xl"
        >
          <ArrowLeft className="size-4" />
        </Button>
      )}
      <Search
        className={cn(
          "pointer-events-none absolute left-3 size-4 text-muted-foreground",
          variant === "dialog" && (compact ? "left-14" : "left-3"),
        )}
      />
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
            "h-14 rounded-none border-0 bg-transparent pr-8 pl-8 text-base shadow-none focus-visible:ring-0 dark:bg-transparent",
          variant === "sidebar" &&
            "h-9 rounded-xl border-transparent bg-sidebar-accent shadow-none focus-visible:border-primary/30 focus-visible:ring-primary/15",
        )}
      />
      {serverState === "pending" && (
        <span className="absolute right-3 flex items-center gap-1 text-xs text-muted-foreground">
          <Loader2 className="size-3 animate-spin" />
          AI…
        </span>
      )}
      {serverState === "failed" && !aiSearchAsYouType && (
        <span className="absolute right-3 text-xs text-destructive">
          AI search failed — press Enter to retry
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
        variant === "dialog" &&
          (compact
            ? "min-h-0 flex-1 overflow-y-auto p-2"
            : "max-h-[min(60vh,32rem)] overflow-y-auto p-2"),
      )}
    >
      {!queryValue && <SectionLabel>Recent</SectionLabel>}
      {rows.map((row, index) => (
        <Fragment key={row.key}>
          {localRows.length > 0 && index === leadingRows.length + primaryRows.length && (
            <SectionLabel>Found locally</SectionLabel>
          )}
          <PaletteResultRow
            row={row}
            optionId={`search-option-${index}`}
            selected={row.key === selectedKey}
            parentPage={row.kind === "content" ? parentPageForRow(row, parentPages) : null}
            showSource={showSearchSources}
            onActivate={activateRow}
            onSelect={setSelectedKey}
          />
        </Fragment>
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
      <div
        className={cn("overflow-hidden", compact ? "flex h-full min-h-0 flex-col" : "rounded-2xl")}
      >
        {input}
        {resultList}
        <div
          className={cn(
            "flex items-center justify-between border-t border-border/60 px-4 text-[11px] text-muted-foreground",
            compact ? "pt-2 pb-[calc(0.5rem+var(--safe-area-inset-bottom))]" : "py-2",
          )}
        >
          <span>{displayedHits.length} results</span>
          {!compact && <span>↑↓ · ⏎ open · ⇧⏎ open beside · esc</span>}
        </div>
      </div>
    );
  }

  if (variant === "dashboard") {
    return (
      <div>
        {input}
        {queryValue && (
          <div className="mt-2 max-h-80 overflow-y-auto rounded-xl bg-background/35 p-1.5">
            {resultList}
          </div>
        )}
      </div>
    );
  }

  if (variant === "sidebar") {
    return (
      <div className="min-h-0">
        {input}
        {queryValue && (
          <div className="mt-2 max-h-[min(50dvh,28rem)] overflow-y-auto rounded-xl bg-background/35 p-1">
            {resultList}
          </div>
        )}
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
