import { useState } from "react";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { getNodeByUuid, getPageByTitle, type Node } from "@/lib/api";

const PREVIEW_CHARS = 240;

type FetchState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "found"; node: Node }
  | { kind: "missing" }
  | { kind: "error"; message: string };

/** Hover preview for `[[Page Title]]`. Lazy-fetches the page on first hover. */
export function WikiLink({ title }: { title: string }) {
  const [state, setState] = useState<FetchState>({ kind: "idle" });

  const onOpenChange = (open: boolean) => {
    if (!open || state.kind !== "idle") return;
    setState({ kind: "loading" });
    getPageByTitle(title)
      .then((node) => setState(node ? { kind: "found", node } : { kind: "missing" }))
      .catch((e) => setState({ kind: "error", message: String(e) }));
  };

  return (
    <HoverCard openDelay={250} closeDelay={100} onOpenChange={onOpenChange}>
      <HoverCardTrigger asChild>
        <span className="cursor-pointer rounded text-sky-600 hover:underline dark:text-sky-400">
          {title}
        </span>
      </HoverCardTrigger>
      <HoverCardContent className="w-80 text-sm">
        <PreviewBody state={state} label={title} kind="page" />
      </HoverCardContent>
    </HoverCard>
  );
}

/** Hover preview for `((block-uuid))`. */
export function BlockRef({ uuid }: { uuid: string }) {
  const [state, setState] = useState<FetchState>({ kind: "idle" });

  const onOpenChange = (open: boolean) => {
    if (!open || state.kind !== "idle") return;
    setState({ kind: "loading" });
    getNodeByUuid(uuid)
      .then((node) => setState(node ? { kind: "found", node } : { kind: "missing" }))
      .catch((e) => setState({ kind: "error", message: String(e) }));
  };

  const broken = state.kind === "missing";

  return (
    <HoverCard openDelay={250} closeDelay={100} onOpenChange={onOpenChange}>
      <HoverCardTrigger asChild>
        <span
          className={
            broken
              ? "cursor-default rounded bg-destructive/10 px-1 font-mono text-[0.85em] text-destructive line-through"
              : "cursor-pointer rounded bg-muted/60 px-1 font-mono text-[0.85em] text-muted-foreground hover:text-foreground"
          }
        >
          (({uuid}))
        </span>
      </HoverCardTrigger>
      <HoverCardContent className="w-80 text-sm">
        <PreviewBody state={state} label={uuid} kind="block" />
      </HoverCardContent>
    </HoverCard>
  );
}

function PreviewBody({
  state,
  label,
  kind,
}: {
  state: FetchState;
  label: string;
  kind: "page" | "block";
}) {
  if (state.kind === "loading" || state.kind === "idle") {
    return <span className="text-muted-foreground">loading…</span>;
  }
  if (state.kind === "error") {
    return <span className="text-destructive">error: {state.message}</span>;
  }
  if (state.kind === "missing") {
    return (
      <span className="text-muted-foreground">
        {kind === "page" ? (
          <>
            <span className="font-medium text-foreground">{label}</span> — stub, will be created on
            save
          </>
        ) : (
          <>broken reference</>
        )}
      </span>
    );
  }
  const { node } = state;
  const preview = node.content.slice(0, PREVIEW_CHARS);
  const truncated = node.content.length > PREVIEW_CHARS;
  return (
    <div className="space-y-1">
      {node.title && <div className="font-medium">{node.title}</div>}
      <div className="whitespace-pre-wrap break-words text-muted-foreground">
        {preview || <span className="italic">empty</span>}
        {truncated && "…"}
      </div>
    </div>
  );
}
