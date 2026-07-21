import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import { RecentJournals } from "./recent-journals";

describe("RecentJournals", () => {
  const journal = {
    uuid: "019f0000-0000-7000-8000-000000000001",
    kind: { kind: "journal" as const, date: "2026-07-17" },
    title: null,
    layout: "outline" as const,
    titleRevision: "test-title-revision",
    createdAt: 0,
    updatedAt: 0,
  };

  it("prevents a second navigation while a journal request is pending", () => {
    const html = renderToStaticMarkup(
      <RecentJournals pages={[journal]} activeUuid={null} busy onOpen={() => undefined} />,
    );

    expect(html).toContain('disabled=""');
    expect(html).toContain("17.07");
  });

  it("fits recent days into the available width without a scrollbar", () => {
    const pages = Array.from({ length: 7 }, (_, index) => ({
      ...journal,
      uuid: `019f0000-0000-7000-8000-00000000000${index}`,
      kind: { kind: "journal" as const, date: `2026-07-${String(17 - index).padStart(2, "0")}` },
    }));
    const html = renderToStaticMarkup(
      <RecentJournals pages={pages} activeUuid={null} busy={false} onOpen={() => undefined} />,
    );

    expect(html).not.toContain("overflow-x-auto");
    expect(html.match(/flex-1/g)).toHaveLength(7);
    expect(html.match(/aria-label="Open journal /g)).toHaveLength(7);
  });
});
