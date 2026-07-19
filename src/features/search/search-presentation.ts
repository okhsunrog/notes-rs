import { contentUuid, type SearchHit } from "@/lib/api";

export type ServerSearchState = "absent" | "idle" | "pending" | "success" | "failed";

export type SearchPresentation = {
  primary: SearchHit[];
  localExtras: SearchHit[];
};

export function presentSearchResults({
  localHits,
  serverHits,
  serverState,
}: {
  localHits: SearchHit[];
  serverHits: SearchHit[];
  serverState: ServerSearchState;
}): SearchPresentation {
  if (serverHits.length === 0 || serverState === "failed" || serverState === "absent") {
    return { primary: localHits, localExtras: [] };
  }

  const serverUuids = new Set(serverHits.map((hit) => contentUuid(hit.content)));
  return {
    primary: serverHits,
    localExtras: localHits.filter((hit) => !serverUuids.has(contentUuid(hit.content))).slice(0, 5),
  };
}
