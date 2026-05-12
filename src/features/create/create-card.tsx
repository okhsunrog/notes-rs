import { useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { NoteEditor, type NoteEditorHandle } from "@/components/note-editor";
import { createNode } from "@/lib/api";

type Props = {
  onStatus: (s: string) => void;
};

export function CreateCard({ onStatus }: Props) {
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const editorRef = useRef<NoteEditorHandle>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!editorRef.current) return;
    const { markdown, json } = await editorRef.current.serialize();
    const plain = markdown.trim();
    if (!plain) return;
    setBusy(true);
    try {
      const n = await createNode({
        kind: "block",
        title: title.trim() || null,
        content: plain,
        contentJson: json,
      });
      onStatus(`created node #${n.id}`);
      setTitle("");
      editorRef.current.reset();
    } catch (err) {
      onStatus(`error: ${err}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Create</CardTitle>
        <CardDescription>Add a block to the graph.</CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={submit} className="space-y-3">
          <Input
            placeholder="title (optional)"
            value={title}
            onChange={(e) => setTitle(e.currentTarget.value)}
          />
          <NoteEditor ref={editorRef} placeholder="write a note…" />
          <Button type="submit" disabled={busy}>
            Create
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}
