import { useState } from "react";
import { Menu } from "@base-ui/react/menu";
import { MoreHorizontal, Pencil, Star, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { refreshPanelAfterClose } from "@/app/eink-refresh";
import { useOverlayInkSuppression } from "@/app/ink-suppression";
import { pageDisplayTitle } from "@/features/journal/journal-date";
import { useWorkspaceController } from "@/features/workspace/workspace-controller";
import type { Page } from "@/lib/api";
import { cn } from "@/lib/utils";
import { usePageNavigationStore } from "./page-navigation-store";
import { RenamePageDialog } from "./rename-page-dialog";

/**
 * The per-row actions of a note list.
 *
 * The lists used to end a row with an arrow, which only repeated what tapping the row already
 * did. The affordance is worth more as a menu: renaming a handwritten note has no other home,
 * and favouriting or deleting one otherwise means opening it first.
 *
 * A menu is an overlay, so opening it holds the BOOX firmware pen down — the items take their
 * taps instead of the sheet taking a stroke through them — and closing it repaints the panel.
 * Items are `role="menuitem"`, so `[data-highlighted]` inverts them on an e-ink display through
 * the shared rule in `index.css` rather than tinting them a few levels of gray.
 */
export function NoteRowMenu({ page, className }: { page: Page; className?: string }) {
  const controller = useWorkspaceController();
  const favorite = usePageNavigationStore((state) => state.favoritePageUuids.includes(page.uuid));
  const toggleFavoritePage = usePageNavigationStore((state) => state.toggleFavoritePage);
  const overlay = useOverlayInkSuppression();
  const [renaming, setRenaming] = useState(false);
  const [renamed, setRenamed] = useState<Page | null>(null);
  const current = renamed ?? page;
  const title = pageDisplayTitle(current);

  return (
    <>
      <Menu.Root onOpenChange={refreshPanelAfterClose(overlay)}>
        <Menu.Trigger
          render={
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label={`More options for ${title}`}
              // The row opens the note; a tap that lands on the menu must not do that as well.
              onClick={(event) => event.stopPropagation()}
              className={cn("shrink-0 rounded-lg text-muted-foreground", className)}
            />
          }
        >
          <MoreHorizontal className="size-4" />
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner sideOffset={6} align="end">
            <Menu.Popup
              // The popup is a React portal, so its clicks still bubble through the row that
              // owns the menu; the row must not treat choosing an item as "open the note".
              onClick={(event) => event.stopPropagation()}
              className="z-50 min-w-44 rounded-xl border bg-popover p-1 text-popover-foreground shadow-panel outline-none"
            >
              <Menu.Item
                className="flex cursor-pointer items-center gap-2 rounded-lg px-3 py-2.5 text-sm outline-none data-[highlighted]:bg-accent"
                onClick={() => setRenaming(true)}
              >
                <Pencil className="size-4" />
                Rename
              </Menu.Item>
              <Menu.Item
                className="flex cursor-pointer items-center gap-2 rounded-lg px-3 py-2.5 text-sm outline-none data-[highlighted]:bg-accent"
                onClick={() => toggleFavoritePage(page.uuid)}
              >
                <Star className={cn("size-4", favorite && "fill-current text-primary")} />
                {favorite ? "Remove from favorites" : "Add to favorites"}
              </Menu.Item>
              <Menu.Item
                className="flex cursor-pointer items-center gap-2 rounded-lg px-3 py-2.5 text-sm text-destructive outline-none data-[highlighted]:bg-accent"
                onClick={() => void controller.onDelete(current)}
              >
                <Trash2 className="size-4" />
                Delete note
              </Menu.Item>
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
      {renaming && (
        <RenamePageDialog
          page={current}
          open
          onOpenChange={setRenaming}
          onSaved={(updated) => {
            setRenamed(updated);
            controller.onSaved(updated);
          }}
        />
      )}
    </>
  );
}
