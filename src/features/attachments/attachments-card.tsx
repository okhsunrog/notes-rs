import { useEffect, useState } from "react";
import { ChevronDown, ExternalLink, File, Loader2, Paperclip, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  attachFile,
  deleteAttachment,
  listAttachments,
  openAttachment,
  type Node,
} from "@/lib/api";

export function AttachmentsCard({
  parentId,
  onStatus,
}: {
  parentId: number;
  onStatus: (message: string) => void;
}) {
  const [attachments, setAttachments] = useState<Node[]>([]);
  const [busy, setBusy] = useState(false);
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    listAttachments(parentId)
      .then((items) => {
        setAttachments(items);
        if (items.length > 0) setExpanded(true);
      })
      .catch((error) => onStatus(`attachment error: ${String(error)}`));
  }, [parentId, onStatus]);

  async function add() {
    setBusy(true);
    try {
      const attachment = await attachFile(parentId);
      if (attachment) {
        setAttachments((current) => [...current, attachment]);
        setExpanded(true);
        onStatus(`Attached ${attachment.title ?? "file"}`);
      }
    } catch (error) {
      onStatus(`attachment error: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  }

  async function remove(attachment: Node) {
    if (
      !window.confirm(
        `Remove attachment “${attachment.title ?? "file"}”? A backup will be created first.`,
      )
    )
      return;
    try {
      if (await deleteAttachment(attachment.id)) {
        setAttachments((current) => current.filter((item) => item.id !== attachment.id));
        onStatus("Attachment removed; a recovery backup was created.");
      }
    } catch (error) {
      onStatus(`attachment error: ${String(error)}`);
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
            <span className="rounded-full bg-muted px-2 py-0.5 text-[10px] text-muted-foreground">
              {attachments.length}
            </span>
          )}
        </button>
        <Button
          variant="ghost"
          size="sm"
          className="rounded-lg text-muted-foreground"
          disabled={busy}
          onClick={() => void add()}
        >
          {busy ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : (
            <Paperclip className="size-3.5" />
          )}{" "}
          Add file
        </Button>
      </div>
      {expanded && (
        <div className="mt-3">
          <p className="mb-3 text-xs text-muted-foreground">
            Files are stored locally and included in portable archives.
          </p>
          {attachments.length === 0 ? (
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
                  key={attachment.id}
                  className="flex items-center gap-2 rounded-xl border border-border/60 bg-card/60 px-2.5 py-2 text-xs"
                >
                  <File className="size-3.5 shrink-0 text-muted-foreground" />
                  <span className="min-w-0 flex-1 truncate">
                    {attachment.title ?? "attachment"}
                  </span>
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label={`Open ${attachment.title ?? "attachment"}`}
                    onClick={() =>
                      void openAttachment(attachment.id).catch((error) =>
                        onStatus(`open error: ${String(error)}`),
                      )
                    }
                  >
                    <ExternalLink className="size-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label={`Remove ${attachment.title ?? "attachment"}`}
                    className="text-muted-foreground hover:text-destructive"
                    onClick={() => void remove(attachment)}
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}
