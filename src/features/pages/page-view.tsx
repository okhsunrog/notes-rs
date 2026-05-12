import { useEffect, useRef, useState } from "react";
import { Save, ArrowLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NoteEditor, type NoteEditorHandle } from "@/components/note-editor";
import { updateNode, type Node } from "@/lib/api";

type Props = {
  page: Node;
  onSaved: (updated: Node) => void;
  onStatus: (s: string) => void;
  onClose: () => void;
};

export function PageView({ page, onSaved, onStatus, onClose }: Props) {
  const [title, setTitle] = useState(page.title ?? "");
  const [busy, setBusy] = useState(false);
  const editorRef = useRef<NoteEditorHandle>(null);

  useEffect(() => {
    setTitle(page.title ?? "");
  }, [page.id, page.title]);

  async function save() {
    if (!editorRef.current) return;
    const { markdown, json } = await editorRef.current.serialize();
    setBusy(true);
    try {
      await updateNode({
        id: page.id,
        title: title.trim() || null,
        content: markdown.trim(),
        contentJson: json,
      });
      onSaved({
        ...page,
        title: title.trim() || null,
        content: markdown.trim(),
        content_json: json,
      });
      onStatus(`saved page #${page.id}`);
    } catch (err) {
      onStatus(`error: ${err}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-4">
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="sm" onClick={onClose} title="Back">
          <ArrowLeft className="size-4" />
        </Button>
        <Input
          value={title}
          onChange={(e) => setTitle(e.currentTarget.value)}
          placeholder="page title"
          className="h-10 border-0 bg-transparent px-0 text-2xl font-semibold shadow-none focus-visible:ring-0"
        />
        <Button onClick={save} disabled={busy} size="sm">
          <Save className="mr-1 size-4" />
          Save
        </Button>
      </div>

      <div className="text-xs text-muted-foreground">
        Node #{page.id} · {page.kind} · updated {new Date(page.updated_at * 1000).toLocaleString()}
      </div>

      <NoteEditor
        key={page.id}
        ref={editorRef}
        initialJson={page.content_json}
        initialPlain={page.content}
        placeholder="start writing…"
      />
    </div>
  );
}
