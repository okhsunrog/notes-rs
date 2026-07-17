import { type Page } from "@/lib/api";
import type { MarkdownOpenHandler } from "@/features/markdown";
import type { PagePresentation } from "@/features/pages/page-presentation";
import { BlockChildren } from "./block-tree";
import { OutlinerProvider } from "./outliner-store";

type Props = {
  page: Page;
  initialEditingUuid?: string | null;
  focusRequest?: number;
  onOpenMarkdownLink: MarkdownOpenHandler;
  presentation?: PagePresentation;
};

export function Outliner({
  page,
  initialEditingUuid = null,
  focusRequest = 0,
  onOpenMarkdownLink,
  presentation = "editing",
}: Props) {
  return (
    <OutlinerProvider
      initialEditingUuid={initialEditingUuid}
      initialEditingRequest={focusRequest}
      layout={page.layout}
      onOpenMarkdownLink={onOpenMarkdownLink}
      readOnly={presentation === "reading"}
    >
      <BlockChildren
        pageUuid={page.uuid}
        parentUuid={null}
        depth={0}
        focusFirstBlockRequest={initialEditingUuid === null ? focusRequest : 0}
        emptyTitle={page.kind.kind === "journal" ? "Nothing captured for this day yet." : undefined}
        emptyActionLabel={page.kind.kind === "journal" ? "Start writing" : undefined}
      />
    </OutlinerProvider>
  );
}
