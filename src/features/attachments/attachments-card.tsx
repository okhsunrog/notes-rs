import { useEffect, useState } from "react";
import { ExternalLink, File, Loader2, Paperclip, Trash2 } from "lucide-react";
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

  useEffect(() => {
    listAttachments(parentId)
      .then(setAttachments)
      .catch((error) => onStatus(`attachment error: ${String(error)}`));
  }, [parentId, onStatus]);

  async function add() {
    setBusy(true);
    try {
      const attachment = await attachFile(parentId);
      if (attachment) {
        setAttachments((current) => [...current, attachment]);
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
    <section className="rounded-md border bg-card/40 p-3">
      <div className="flex items-center justify-between gap-2">
        <div>
          <h2 className="text-sm font-medium">Attachments</h2>
          <p className="text-xs text-muted-foreground">
            Files are copied into app data and included in archives.
          </p>
        </div>
        <Button variant="outline" size="sm" disabled={busy} onClick={() => void add()}>
          {busy ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : (
            <Paperclip className="size-3.5" />
          )}{" "}
          Add file
        </Button>
      </div>
      {attachments.length > 0 && (
        <ul className="mt-3 space-y-1">
          {attachments.map((attachment) => (
            <li
              key={attachment.id}
              className="flex items-center gap-2 rounded border bg-background px-2 py-1.5 text-xs"
            >
              <File className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate">{attachment.title ?? "attachment"}</span>
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
    </section>
  );
}
