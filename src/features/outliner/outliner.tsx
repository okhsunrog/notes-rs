import { type Page } from "@/lib/api";
import type { PagePresentation } from "@/features/pages/page-presentation";
import { BlockChildren } from "./block-tree";
import { OutlinerProvider } from "./outliner-store";

type Props = {
  page: Page;
  initialEditingUuid?: string | null;
  focusRequest?: number;
  presentation?: PagePresentation;
};

export function Outliner({
  page,
  initialEditingUuid = null,
  focusRequest = 0,
  presentation = "editing",
}: Props) {
  return (
    <OutlinerProvider
      initialEditingUuid={initialEditingUuid}
      initialEditingRequest={focusRequest}
      layout={page.layout}
      readOnly={presentation === "reading"}
    >
      <BlockChildren
        pageUuid={page.uuid}
        parentUuid={null}
        depth={0}
        focusFirstBlockRequest={initialEditingUuid === null ? focusRequest : 0}
      />
    </OutlinerProvider>
  );
}
