import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowRight, Clock3, FilePlus2, FileText, ListFilter, Search, Star, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { NewNoteButton } from "@/features/handwriting/new-note-button";
import { PageIcon } from "./page-icon";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger } from "@/components/ui/select";
import { pageDisplayTitle } from "@/features/journal/journal-date";
import { useWorkspaceController } from "@/features/workspace/workspace-controller";
import { dispositionFromShiftKey } from "@/features/workspace/workspace-model";
import { listPages, type Page } from "@/lib/api";
import { notifyError } from "@/lib/notify";
import { queryKeys } from "@/lib/query";
import { cn } from "@/lib/utils";
import { usePageNavigationStore } from "./page-navigation-store";
import {
  presentAllNotes,
  type AllNotesLayout,
  type AllNotesScope,
  type AllNotesSort,
} from "./all-notes-model";

const SORT_OPTIONS: readonly { value: AllNotesSort; label: string }[] = [
  { value: "updated", label: "Recently edited" },
  { value: "opened", label: "Recently opened" },
  { value: "created", label: "Recently created" },
  { value: "title", label: "Title A–Z" },
];

export function AllNotesView() {
  const controller = useWorkspaceController();
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState<AllNotesScope>("all");
  const [layout, setLayout] = useState<AllNotesLayout>("all");
  const [sort, setSort] = useState<AllNotesSort>("updated");
  const favoritePageUuids = usePageNavigationStore((state) => state.favoritePageUuids);
  const recentPageUuids = usePageNavigationStore((state) => state.recentPageUuids);
  const toggleFavoritePage = usePageNavigationStore((state) => state.toggleFavoritePage);
  const pagesQuery = useQuery({
    queryKey: queryKeys.pageList("notes", 10_000),
    queryFn: () => listPages({ filter: "notes", limit: 10_000 }),
  });
  const pages = pagesQuery.data ?? [];
  const visiblePages = useMemo(
    () =>
      presentAllNotes(pages, {
        query,
        scope,
        layout,
        sort,
        favoritePageUuids,
        recentPageUuids,
      }),
    [favoritePageUuids, layout, pages, query, recentPageUuids, scope, sort],
  );

  useEffect(() => {
    if (pagesQuery.error) notifyError("all notes", pagesQuery.error);
  }, [pagesQuery.error]);

  const filtersActive = scope !== "all" || layout !== "all";

  return (
    <div className="mx-auto min-h-full max-w-5xl px-5 pt-7 pb-[calc(1.75rem+var(--safe-area-inset-bottom))] sm:px-8 sm:py-9">
      <header className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="text-xs font-semibold tracking-[0.16em] text-primary uppercase">Library</p>
          <h1 className="mt-1 text-2xl font-semibold tracking-[-0.035em] sm:text-3xl">All notes</h1>
          <p className="mt-1 text-xs text-muted-foreground">
            {visiblePages.length === pages.length
              ? `${pages.length} notes`
              : `${visiblePages.length} of ${pages.length} notes`}
          </p>
        </div>
        <NewNoteButton
          className="brand-button h-9 rounded-xl px-3.5"
          onClick={() => void controller.createNewNote()}
        >
          <FilePlus2 className="size-3.5" />
          New note
        </NewNoteButton>
      </header>

      <div className="mt-6 rounded-2xl border border-border/60 bg-card/45 p-3 shadow-sm">
        <div className="flex flex-col gap-2.5 sm:flex-row">
          <div className="relative min-w-0 flex-1">
            <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={query}
              onChange={(event) => setQuery(event.currentTarget.value)}
              placeholder="Search note titles…"
              aria-label="Search note titles"
              className="h-10 rounded-xl bg-background/70 pr-9 pl-9"
            />
            {query && (
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                aria-label="Clear title search"
                onClick={() => setQuery("")}
                className="absolute top-1/2 right-2 -translate-y-1/2 rounded-lg"
              >
                <X className="size-3.5" />
              </Button>
            )}
          </div>
          <Select
            value={sort}
            onValueChange={(value) => {
              if (value !== null) setSort(value);
            }}
          >
            <SelectTrigger
              size="sm"
              aria-label="Sort notes"
              className="h-10 min-w-44 rounded-xl bg-background/70 px-3"
            >
              <Clock3 className="size-3.5" />
              <span className="text-xs">
                {SORT_OPTIONS.find((option) => option.value === sort)?.label}
              </span>
            </SelectTrigger>
            <SelectContent align="end" className="min-w-48 rounded-xl p-1">
              {SORT_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value} className="rounded-lg">
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="mt-3 flex flex-wrap items-center gap-2 border-t border-border/50 pt-3">
          <ListFilter className="mr-0.5 size-3.5 text-muted-foreground" />
          <FilterButton active={scope === "all"} onClick={() => setScope("all")}>
            All
          </FilterButton>
          <FilterButton active={scope === "favorites"} onClick={() => setScope("favorites")}>
            <Star className="size-3" /> Favorites
          </FilterButton>
          <span className="mx-1 h-4 w-px bg-border" />
          {(["all", "outline", "document"] as const).map((value) => (
            <FilterButton key={value} active={layout === value} onClick={() => setLayout(value)}>
              {value === "all" ? "Any layout" : capitalize(value)}
            </FilterButton>
          ))}
          {filtersActive && (
            <Button
              type="button"
              variant="ghost"
              size="xs"
              onClick={() => {
                setScope("all");
                setLayout("all");
              }}
              className="ml-auto rounded-lg text-muted-foreground"
            >
              Reset filters
            </Button>
          )}
        </div>
      </div>

      <section
        aria-label="Notes"
        className="mt-4 overflow-hidden rounded-2xl border border-border/60 bg-card/35 shadow-sm"
      >
        {pagesQuery.isPending ? (
          <p className="px-5 py-12 text-center text-sm text-muted-foreground">Loading notes…</p>
        ) : visiblePages.length === 0 ? (
          <div className="px-5 py-14 text-center">
            <FileText className="mx-auto size-7 text-muted-foreground/50" />
            <p className="mt-3 text-sm font-medium">No matching notes</p>
            <p className="mt-1 text-xs text-muted-foreground">
              {pages.length === 0
                ? "Create your first note to begin."
                : "Try another title or filter."}
            </p>
          </div>
        ) : (
          <ul className="divide-y divide-border/45">
            {visiblePages.map((page) => (
              <AllNotesRow
                key={page.uuid}
                page={page}
                favorite={favoritePageUuids.includes(page.uuid)}
                onOpen={(shiftKey) =>
                  void controller.openContent(
                    { kind: "page", record: page },
                    dispositionFromShiftKey(shiftKey),
                  )
                }
                onToggleFavorite={() => toggleFavoritePage(page.uuid)}
              />
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function FilterButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Button
      type="button"
      variant={active ? "secondary" : "ghost"}
      size="xs"
      aria-pressed={active}
      onClick={onClick}
      className="rounded-lg"
    >
      {children}
    </Button>
  );
}

function AllNotesRow({
  page,
  favorite,
  onOpen,
  onToggleFavorite,
}: {
  page: Page;
  favorite: boolean;
  onOpen: (shiftKey: boolean) => void;
  onToggleFavorite: () => void;
}) {
  return (
    <li className="group flex min-w-0 items-center transition-colors hover:bg-accent/45">
      <button
        type="button"
        onClick={(event) => onOpen(event.shiftKey)}
        className="flex min-w-0 flex-1 items-center gap-3 px-4 py-3 text-left sm:px-5"
      >
        <span className="flex size-9 shrink-0 items-center justify-center rounded-xl bg-primary/8 text-primary">
          <PageIcon page={page} className="size-4" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm font-medium">{pageDisplayTitle(page)}</span>
          <span className="mt-0.5 flex items-center gap-2 text-[10px] text-muted-foreground">
            <span className="capitalize">{page.layout}</span>
            <span aria-hidden="true">·</span>
            <span>Edited {formatDate(page.updatedAt)}</span>
          </span>
        </span>
        <ArrowRight className="size-4 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100" />
      </button>
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        aria-label={
          favorite
            ? `Remove ${pageDisplayTitle(page)} from favorites`
            : `Add ${pageDisplayTitle(page)} to favorites`
        }
        onClick={onToggleFavorite}
        className={cn("mr-3 rounded-lg text-muted-foreground", favorite && "text-primary")}
      >
        <Star className={cn("size-3.5", favorite && "fill-current")} />
      </Button>
    </li>
  );
}

function formatDate(timestamp: number): string {
  return new Date(timestamp * 1000).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year:
      new Date(timestamp * 1000).getFullYear() === new Date().getFullYear() ? undefined : "numeric",
  });
}

function capitalize(value: string): string {
  return value.charAt(0).toUpperCase() + value.slice(1);
}
