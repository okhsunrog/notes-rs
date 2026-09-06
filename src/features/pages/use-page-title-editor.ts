import { useCallback, useEffect, useRef, useState } from "react";
import { CommandFailure, getPage, renamePage, type Page } from "@/lib/api";
import { DebouncedAction } from "@/lib/debounced-action";
import { notifyError } from "@/lib/notify";
import { usePageSessionRegistry, useTitleDraftOverlay } from "./page-session";

export type TitleSaveState = "idle" | "dirty" | "saving" | "saved" | "error";

const AUTOSAVE_MS = 400;

/**
 * The window-local title draft, its debounced rename and the replica conflict
 * choice, without the block/document editing a text page also carries.
 */
export function usePageTitleEditor(page: Page, canEdit: boolean, onSaved: (updated: Page) => void) {
  const sessions = usePageSessionRegistry();
  const overlay = useTitleDraftOverlay(page.uuid);
  const [saveState, setSaveState] = useState<TitleSaveState>("idle");
  const autosave = useRef(new DebouncedAction()).current;
  const pageRef = useRef(page);
  const inFlight = useRef<Promise<boolean> | null>(null);
  const onSavedRef = useRef(onSaved);
  onSavedRef.current = onSaved;

  const flush = useCallback(async (): Promise<boolean> => {
    autosave.cancel();
    if (inFlight.current) return inFlight.current;
    const pending = (async () => {
      // A keystroke landing during the rename leaves a fresh draft behind; keep
      // saving until the overlay is empty so nothing waits for another edit.
      while (true) {
        const current = pageRef.current;
        const attempt = sessions.beginTitleSave(current.uuid);
        if (!attempt) {
          const draft = sessions.getSnapshot(current.uuid).title;
          if (draft?.conflict || draft?.inFlight) {
            setSaveState(draft.conflict ? "error" : "saving");
            return false;
          }
          setSaveState("idle");
          return true;
        }
        const nextTitle = attempt.draft.trim() || null;
        setSaveState("saving");
        try {
          if (nextTitle === (current.title ?? null)) {
            sessions.acknowledgeTitleSave(current.uuid, attempt, {
              text: current.title ?? "",
              revision: current.titleRevision,
            });
          } else {
            const updated = await renamePage(current.uuid, nextTitle, attempt.expectedRevision);
            pageRef.current = updated;
            sessions.acknowledgeTitleSave(current.uuid, attempt, {
              text: updated.title ?? "",
              revision: updated.titleRevision,
            });
            onSavedRef.current(updated);
          }
          if (sessions.getSnapshot(current.uuid).title) continue;
          setSaveState("saved");
          return true;
        } catch (error) {
          sessions.failTitleSave(current.uuid, attempt);
          if (error instanceof CommandFailure && error.code === "conflict") {
            try {
              const latest = await getPage(current.uuid);
              if (latest) {
                pageRef.current = latest;
                onSavedRef.current(latest);
                sessions.acceptTitleSnapshot(current.uuid, {
                  text: latest.title ?? "",
                  revision: latest.titleRevision,
                });
              }
            } catch (refreshError) {
              console.error("title conflict refresh failed", refreshError);
            }
          }
          setSaveState("error");
          notifyError("save", error);
          return false;
        }
      }
    })();
    inFlight.current = pending;
    try {
      return await pending;
    } finally {
      if (inFlight.current === pending) inFlight.current = null;
    }
  }, [autosave, sessions]);

  const edit = useCallback(
    (draft: string) => {
      if (!canEdit) return;
      sessions.editTitle(page.uuid, draft.replace(/[\r\n]+/g, " "), {
        text: pageRef.current.title ?? "",
        revision: pageRef.current.titleRevision,
      });
      setSaveState("dirty");
      autosave.schedule(() => void flush(), AUTOSAVE_MS);
    },
    [autosave, canEdit, flush, page.uuid, sessions],
  );

  useEffect(() => {
    pageRef.current = page;
    sessions.acceptTitleSnapshot(page.uuid, {
      text: page.title ?? "",
      revision: page.titleRevision,
    });
  }, [page, sessions]);

  useEffect(() => {
    if (overlay?.conflict) setSaveState("error");
    else if (overlay?.inFlight) setSaveState("saving");
    else if (overlay) setSaveState((state) => (state === "error" ? state : "dirty"));
    else setSaveState((state) => (state === "saved" ? state : "idle"));
  }, [overlay]);

  useEffect(() => {
    return () => {
      if (autosave.cancel()) void flush();
    };
  }, [autosave, flush]);

  return {
    title: overlay?.draft ?? page.title ?? "",
    conflict: overlay?.conflict != null,
    saveState,
    edit,
    flush,
    useRemote: useCallback(() => sessions.useRemoteTitle(page.uuid), [page.uuid, sessions]),
    keepLocal: useCallback(() => {
      sessions.keepLocalTitle(page.uuid);
      setSaveState("dirty");
      autosave.schedule(() => void flush(), AUTOSAVE_MS);
    }, [autosave, flush, page.uuid, sessions]),
  };
}
