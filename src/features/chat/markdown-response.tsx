import {
  MarkdownRenderer,
  type MarkdownOpenHandler,
  useAttachmentImageResolver,
} from "@/features/markdown";

const ASSISTANT_CONTEXT = { kind: "assistant" } as const;

export default function MarkdownResponse({
  children,
  onOpenLink,
}: {
  children: string;
  onOpenLink: MarkdownOpenHandler;
}) {
  const resolveImage = useAttachmentImageResolver(children);
  return (
    <MarkdownRenderer
      className="assistant-markdown text-[13px] leading-relaxed"
      context={ASSISTANT_CONTEXT}
      markdown={children}
      onOpenLink={onOpenLink}
      resolveImage={resolveImage}
    />
  );
}
