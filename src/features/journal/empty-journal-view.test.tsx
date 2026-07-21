import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import { EmptyJournalView } from "./empty-journal-view";

describe("EmptyJournalView", () => {
  it("previews a missing date without forcing the mobile keyboard open", () => {
    const html = renderToStaticMarkup(
      <EmptyJournalView
        date="2026-07-17"
        busy={false}
        onCapture={async () => true}
        onOpenAllNotes={() => undefined}
        onOpenDate={() => undefined}
      />,
    );

    expect(html).toContain("This date is only being previewed");
    expect(html).toContain('aria-label="Write the first entry for journal 2026-07-17"');
    expect(html).not.toContain("autofocus");
  });

  it("exposes its pending state and disables capture controls", () => {
    const html = renderToStaticMarkup(
      <EmptyJournalView
        date="2026-07-17"
        busy
        onCapture={async () => true}
        onOpenAllNotes={() => undefined}
        onOpenDate={() => undefined}
      />,
    );

    expect(html).toContain('aria-busy="true"');
    expect(html.match(/disabled=""/g)?.length).toBeGreaterThanOrEqual(3);
  });
});
