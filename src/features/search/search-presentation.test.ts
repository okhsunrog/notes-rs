import { describe, expect, it } from "vitest";
import type { SearchHit } from "@/lib/api";
import { presentSearchResults } from "./search-presentation";

function hit(uuid: string): SearchHit {
  return {
    content: {
      kind: "page",
      record: {
        uuid,
        kind: { kind: "note" },
        title: uuid,
        layout: "outline",
        titleRevision: "0000000000000000-00000000-00000000000000000000000000000001",
        createdAt: 0,
        updatedAt: 0,
      },
    },
    score: 1,
    snippet: null,
  };
}

describe("presentSearchResults", () => {
  const local = [hit("local-1"), hit("shared"), hit("local-2")];

  it("uses local results when no server is configured", () => {
    expect(
      presentSearchResults({ localHits: local, serverHits: [], serverState: "absent" }),
    ).toEqual({ primary: local, localExtras: [] });
  });

  it("keeps local results while the server is pending or failed", () => {
    expect(
      presentSearchResults({ localHits: local, serverHits: [], serverState: "pending" }),
    ).toEqual({ primary: local, localExtras: [] });
    expect(
      presentSearchResults({
        localHits: local,
        serverHits: [hit("stale-server")],
        serverState: "failed",
      }),
    ).toEqual({ primary: local, localExtras: [] });
  });

  it("uses local results when the authoritative server list is empty", () => {
    expect(
      presentSearchResults({ localHits: local, serverHits: [], serverState: "success" }),
    ).toEqual({ primary: local, localExtras: [] });
  });

  it("preserves server order and appends at most five UUID-deduplicated local extras", () => {
    const server = [hit("server-2"), hit("shared"), hit("server-1")];
    const manyLocal = [
      hit("shared"),
      hit("local-1"),
      hit("local-2"),
      hit("local-3"),
      hit("local-4"),
      hit("local-5"),
      hit("local-6"),
    ];
    const presented = presentSearchResults({
      localHits: manyLocal,
      serverHits: server,
      serverState: "success",
    });
    expect(presented.primary).toEqual(server);
    expect(presented.localExtras.map((item) => item.content.record.uuid)).toEqual([
      "local-1",
      "local-2",
      "local-3",
      "local-4",
      "local-5",
    ]);
  });
});
