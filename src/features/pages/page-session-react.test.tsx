import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import {
  PageSessionProvider,
  PageSessionRegistry,
  useBlockDraftOverlay,
  useDocumentDraftOverlay,
  useTitleDraftOverlay,
} from "./page-session";
import { encodeDocument } from "@/features/document/document-codec";

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

function DocumentReadingProjection({ pageUuid }: { pageUuid: string }) {
  const document = useDocumentDraftOverlay(pageUuid);
  return <article data-document-reading>{document?.draft ?? "Persisted document"}</article>;
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

  it("projects an exact dirty document buffer into a linked Reading pane", () => {
    const registry = new PageSessionRegistry();
    const base = encodeDocument([
      {
        uuid: "block",
        parentUuid: null,
        style: { kind: "paragraph" },
        markdown: "Persisted document",
      },
    ]);
    registry.editDocument("page", "# In-memory\n\n**before autosave**", {
      buffer: base.markdown,
      sourceMap: base.sourceMap,
      revision: "d1",
    });

    const html = renderToStaticMarkup(
      <PageSessionProvider registry={registry}>
        <DocumentReadingProjection pageUuid="page" />
      </PageSessionProvider>,
    );

    expect(html).toContain("# In-memory\n\n**before autosave**");
    expect(html).not.toContain("Persisted document");
  });
});
