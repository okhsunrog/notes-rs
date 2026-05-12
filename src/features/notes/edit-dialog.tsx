import { useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { NoteEditor, type NoteEditorHandle } from "@/components/note-editor";
import { updateNode, type Node } from "@/lib/api";

type Props = {
  node: Node | null;
  onClose: () => void;
  onSaved: (updated: Node) => void;
  onStatus: (s: string) => void;
};

export function EditDialog({ node, onClose, onSaved, onStatus }: Props) {
  const [title, setTitle] = useState(node?.title ?? "");
  const [busy, setBusy] = useState(false);
  const editorRef = useRef<NoteEditorHandle>(null);

  async function save() {
    if (!node || !editorRef.current) return;
    const { markdown, json } = await editorRef.current.serialize();
    setBusy(true);
    try {
      await updateNode({
        id: node.id,
        title: title.trim() || null,
        content: markdown.trim(),
        contentJson: json,
      });
      onStatus(`updated node #${node.id}`);
      onSaved({
        ...node,
        title: title.trim() || null,
        content: markdown.trim(),
        content_json: json,
      });
    } catch (err) {
      onStatus(`error: ${err}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={!!node} onOpenChange={(v) => !v && onClose()}>
      <DialogContent className="max-w-3xl">
        <DialogHeader>
          <DialogTitle>Edit note</DialogTitle>
          <DialogDescription>
            {node && (
              <>
                Node #{node.id} · {node.kind}
              </>
            )}
          </DialogDescription>
        </DialogHeader>
        {node && (
          <div className="space-y-3">
            <Input
              placeholder="title (optional)"
              value={title}
              onChange={(e) => setTitle(e.currentTarget.value)}
            />
            <NoteEditor
              key={node.id}
              ref={editorRef}
              initialJson={node.content_json}
              initialPlain={node.content}
              placeholder="content…"
            />
          </div>
        )}
        <DialogFooter>
          <Button variant="outline" type="button" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="button" onClick={save} disabled={busy}>
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
