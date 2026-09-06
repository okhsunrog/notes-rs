import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import type { Page } from "@/lib/api";
import { usePageTitleEditor } from "./use-page-title-editor";

/**
 * Renaming a note from a dialog rather than a field in the page chrome.
 *
 * On a BOOX panel the pen is drawn by the firmware into a screen region that sits above every
 * window, so a text field next to the sheet puts the soft keyboard under an armed pen: the taps
 * never arrive and the ink paints over the keys. A dialog registers as an overlay and holds the
 * pen down for as long as it is open, which is also how the stock Notes app renames a note.
 *
 * The save path is the ordinary one, replica conflicts included.
 */
export function RenamePageDialog({
  page,
  open,
  onOpenChange,
  onSaved,
}: {
  page: Page;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved: (updated: Page) => void;
}) {
  const title = usePageTitleEditor(page, true, onSaved);
  const [saving, setSaving] = useState(false);
  // The field edits a local draft. The shared title editor autosaves after a short pause and
  // hands back the trimmed, persisted title, which on a slow keyboard replaces the text under
  // the user's fingers and swallows a trailing space. Nothing leaves the dialog until Save.
  const [draft, setDraft] = useState<string | null>(null);
  const value = draft ?? title.title;

  const save = async () => {
    if (saving) return;
    setSaving(true);
    try {
      if (draft !== null) title.edit(draft);
      // A refused rename keeps the dialog open with the draft and its error in place.
      if (await title.flush()) {
        setDraft(null);
        onOpenChange(false);
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) setDraft(null);
        onOpenChange(next);
      }}
    >
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>Rename note</DialogTitle>
          <DialogDescription>The name is how this note appears in your lists.</DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
          className="space-y-3"
        >
          <Input
            autoFocus
            value={value}
            onChange={(event) => setDraft(event.currentTarget.value)}
            placeholder="Untitled note"
            aria-label="Note name"
          />
          {title.conflict && (
            <div role="alert" className="space-y-2 rounded-lg bg-amber-500/10 p-2 text-xs">
              <p>This title changed on another replica. Choose which version to keep.</p>
              <div className="flex gap-2">
                <Button type="button" variant="outline" size="xs" onClick={title.useRemote}>
                  Use remote
                </Button>
                <Button type="button" size="xs" onClick={title.keepLocal}>
                  Keep mine
                </Button>
              </div>
            </div>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => {
                setDraft(null);
                onOpenChange(false);
              }}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={saving}>
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
