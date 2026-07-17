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
});
