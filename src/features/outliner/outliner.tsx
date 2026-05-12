import { type Node } from "@/lib/api";
import { BlockChildren } from "./block-tree";
import { OutlinerProvider } from "./outliner-store";

type Props = {
  page: Node;
};

export function Outliner({ page }: Props) {
  return (
    <OutlinerProvider>
      <BlockChildren parent={page} depth={0} />
    </OutlinerProvider>
  );
}
