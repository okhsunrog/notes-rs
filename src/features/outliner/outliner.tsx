import { type Page } from "@/lib/api";
import { BlockChildren } from "./block-tree";
import { OutlinerProvider } from "./outliner-store";

type Props = {
  page: Page;
  initialEditingUuid?: string | null;
  focusRequest?: number;
};

export function Outliner({ page, initialEditingUuid = null, focusRequest = 0 }: Props) {
  return (
    <OutlinerProvider
      initialEditingUuid={initialEditingUuid}
      initialEditingRequest={focusRequest}
      view={page.defaultView}
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
