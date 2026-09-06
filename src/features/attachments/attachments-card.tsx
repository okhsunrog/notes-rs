import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, ExternalLink, File, Paperclip, Trash2 } from "lucide-react";
import { useConfirmation } from "@/app/confirmation";
import { Button } from "@/components/ui/button";
import { notifyError, notifyInfo, notifySuccess } from "@/lib/notify";
import {
  attachFile,
  deleteAttachment,
  listAttachments,
  openAttachment,
  type Attachment,
  type AttachmentOwner,
} from "@/lib/api";
import { queryKeys } from "@/lib/query";
import { BusyIndicator } from "@/components/ui/busy-indicator";

export function AttachmentsCard({
  location,
  readOnly = false,
}: {
  location: AttachmentOwner;
  readOnly?: boolean;
}) {
  const confirm = useConfirmation();
  const queryClient = useQueryClient();
  const readOnlyRef = useRef(readOnly);
  readOnlyRef.current = readOnly;
  const [busy, setBusy] = useState(false);
  const [expanded, setExpanded] = useState(false);

  const attachmentsQuery = useQuery<Attachment[]>({
    queryKey: queryKeys.attachments(location.uuid),
    queryFn: () => listAttachments(location),
  });
  const attachments = attachmentsQuery.data ?? [];

  useEffect(() => {
    if (attachments.length > 0) setExpanded(true);
  }, [attachments.length]);

  useEffect(() => {
    if (attachmentsQuery.error) {
      notifyError("attachments", attachmentsQuery.error);
    }
  }, [attachmentsQuery.error]);

  async function add() {
    if (readOnlyRef.current) return;
    setBusy(true);
    try {
      const attachment = await attachFile(location);
      if (attachment) {
        // The native picker can outlive the pane's Editing presentation. Undo
        // the just-created relation if write authority was lost while it was open.
        if (readOnlyRef.current) {
          await deleteAttachment(attachment.uuid);
          await queryClient.invalidateQueries({ queryKey: queryKeys.attachments(location.uuid) });
          notifyInfo("Attachment was not added because this pane is now read-only.");
          return;
        }
        await queryClient.invalidateQueries({ queryKey: queryKeys.attachments(location.uuid) });
        setExpanded(true);
        notifySuccess(`Attached ${attachment.filename}`);
      }
    } catch (error) {
      notifyError("attachment", error);
    } finally {
      setBusy(false);
    }
  }

  async function remove(attachment: Attachment) {
    if (readOnlyRef.current) return;
    if (
      !(await confirm({
        title: "Remove attachment?",
        description: `“${attachment.filename}” will be detached from this note. A recovery backup is created first.`,
        confirmLabel: "Remove attachment",
        destructive: true,
      }))
    )
      return;
    if (readOnlyRef.current) return;
    try {
      if (await deleteAttachment(attachment.uuid)) {
        await queryClient.invalidateQueries({ queryKey: queryKeys.attachments(location.uuid) });
        notifySuccess("Attachment removed; a recovery backup was created.");
      }
    } catch (error) {
      notifyError("attachment", error);
    }
  }

  return (
    <section className="border-t border-border/60 pt-5">
      <div className="flex items-center justify-between gap-2">
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          className="flex items-center gap-2 text-sm font-medium transition hover:text-primary"
        >
          <ChevronDown
            className={`size-3.5 text-muted-foreground transition ${expanded ? "" : "-rotate-90"}`}
          />
          Attachments
          {attachments.length > 0 && (
            <span className="rounded-full bg-muted px-2 py-0.5 text-[10px] eink:text-xs text-muted-foreground">
              {attachments.length}
            </span>
          )}
        </button>
        {!readOnly && (
          <Button
            variant="ghost"
            size="sm"
            className="rounded-lg text-muted-foreground"
            disabled={busy}
            onClick={() => void add()}
          >
            {busy ? (
              <BusyIndicator className="size-3.5" label="Adding file" hideLabel />
            ) : (
              <Paperclip className="size-3.5" />
            )}{" "}
            Add file
          </Button>
        )}
      </div>
      {expanded && (
        <div className="mt-3">
          <p className="mb-3 text-xs text-muted-foreground">
            Files are stored locally and included in portable archives.
          </p>
          {attachments.length === 0 && !readOnly ? (
            <button
              type="button"
              onClick={() => void add()}
              className="w-full rounded-xl border border-dashed border-border/70 p-4 text-center text-xs text-muted-foreground transition hover:border-primary/30 hover:bg-primary/5 hover:text-primary"
            >
              Drop in a reference file
            </button>
          ) : (
            <ul className="space-y-1.5">
              {attachments.map((attachment) => (
                <li
                  key={attachment.uuid}
                  className="flex items-center gap-2 rounded-xl border border-border/60 surface-card px-2.5 py-2 text-xs"
                >
                  <File className="size-3.5 shrink-0 text-muted-foreground" />
                  <span className="min-w-0 flex-1 truncate">{attachment.filename}</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label={`Open ${attachment.filename}`}
                    onClick={() =>
                      void openAttachment(attachment.uuid).catch((error) =>
                        notifyError("open", error),
                      )
                    }
                  >
                    <ExternalLink className="size-3.5" />
                  </Button>
                  {!readOnly && (
                    <Button
                      variant="ghost"
                      size="sm"
                      aria-label={`Remove ${attachment.filename}`}
                      className="text-muted-foreground hover:text-destructive"
                      onClick={() => void remove(attachment)}
                    >
                      <Trash2 className="size-3.5" />
                    </Button>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}
