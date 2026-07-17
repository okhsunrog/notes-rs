import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import {
  PageSessionProvider,
  PageSessionRegistry,
  useBlockDraftOverlay,
  useTitleDraftOverlay,
} from "./page-session";

function ReadingProjection({
  pageUuid,
  persistedTitle,
  persistedBlock,
}: {
  pageUuid: string;
  persistedTitle: string;
  persistedBlock: string;
}) {
  const title = useTitleDraftOverlay(pageUuid);
  const block = useBlockDraftOverlay(pageUuid, "block");
  return (
    <article data-presentation="reading">
      <h1>{title?.draft ?? persistedTitle}</h1>
      <p>{block?.draft ?? persistedBlock}</p>
    </article>
  );
}

function renderReading(registry: PageSessionRegistry) {
  return renderToStaticMarkup(
    <PageSessionProvider registry={registry}>
      <ReadingProjection
        pageUuid="page"
        persistedTitle="Persisted title"
        persistedBlock="Persisted block"
      />
    </PageSessionProvider>,
  );
}

describe("PageSessionProvider", () => {
  it("projects exact in-memory drafts into a newly mounted Reading surface", () => {
    const registry = new PageSessionRegistry();
    registry.editTitle("page", "Title before autosave", {
      text: "Persisted title",
      revision: "t1",
    });
    registry.editBlock("page", "block", "Block before autosave", {
      text: "Persisted block",
      revision: "b1",
    });

    const firstReadingMount = renderReading(registry);
    const remountedReading = renderReading(registry);

    expect(firstReadingMount).toContain("Title before autosave");
    expect(firstReadingMount).toContain("Block before autosave");
    expect(firstReadingMount).not.toContain(">Persisted title<");
    expect(remountedReading).toBe(firstReadingMount);
  });
});
