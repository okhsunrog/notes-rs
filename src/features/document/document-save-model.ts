import { DocumentCodec, type DocumentSourceMap } from "./document-codec";
import type { DocumentUnitDraft } from "@/lib/api";

export function documentUnitsForSave(
  buffer: string,
  sourceMap: DocumentSourceMap,
): DocumentUnitDraft[] {
  return DocumentCodec.reconcile(buffer, sourceMap).units.map((unit) => ({
    previousUuid: unit.previousUuid,
    parentIndex: unit.parentIndex,
    style: unit.style,
    markdown: unit.markdown,
  }));
}
