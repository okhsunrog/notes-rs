import { type Page } from "@/lib/api";
import type { MarkdownOpenHandler } from "@/features/markdown";
import { PagePresentation } from "@/features/pages/page-presentation";
import { BlockChildren } from "./block-tree";
import { OutlinerProvider } from "./outliner-store";

type Props = {
  page: Page;
  initialEditingUuid?: string | null;
  focusRequest?: number;
  onOpenMarkdownLink: MarkdownOpenHandler;
  presentation?: PagePresentation;
  readOnly?: boolean;
};

export function Outliner({
  page,
  initialEditingUuid = null,
  focusRequest = 0,
  onOpenMarkdownLink,
  presentation = PagePresentation.Editing,
  readOnly = presentation === PagePresentation.Reading,
}: Props) {
  return (
    <OutlinerProvider
      initialEditingUuid={initialEditingUuid}
      initialEditingRequest={focusRequest}
      layout={page.layout}
      onOpenMarkdownLink={onOpenMarkdownLink}
      readOnly={readOnly}
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
