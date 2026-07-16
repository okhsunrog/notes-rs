import { type Node } from "@/lib/api";
import { BlockChildren } from "./block-tree";
import { OutlinerProvider } from "./outliner-store";

type Props = {
  page: Node;
  initialEditingId?: number | null;
};

export function Outliner({ page, initialEditingId }: Props) {
  return (
    <OutlinerProvider initialEditingId={initialEditingId}>
      <BlockChildren parent={page} depth={0} />
    </OutlinerProvider>
  );
}
