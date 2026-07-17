import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";
import type { LogseqImportPreview } from "@/lib/api";
import { LogseqImportSectionPresentation, LogseqImportSectionView } from "./logseq-import-section";
import type { LogseqImportState } from "./logseq-import-state";

function preview(): LogseqImportPreview {
  return {
    sessionUuid: "019f0000-0000-7000-8000-000000000001",
    sourceName: "Project notes",
    manifestSha256: "b".repeat(64),
    destination: "not_empty",
    blockers: ["non_empty_workspace"],
    canCommit: false,
    report: {
      pageCount: 42,
      journalCount: 8,
      blockCount: 512,
      syntheticPreambleBlockCount: 1,
      taskCount: 20,
      referenceCount: 64,
      resolvedReferenceCount: 60,
      unresolvedReferenceCount: 4,
      mediaReferenceCount: 14,
      markdownImageCount: 10,
      drawingConversionState: "absent",
      preparedExcalidrawCount: 0,
      preservedExcalidrawCount: 3,
      localMediaReferenceCount: 10,
      inlineMediaReferenceCount: 1,
      blockedRemoteMediaReferenceCount: 2,
      missingMediaReferenceCount: 1,
      blockedUnsafeMediaReferenceCount: 0,
      unsupportedMediaReferenceCount: 0,
      unreferencedAssetCount: 2,
      unreferencedDrawingCount: 3,
      preservedBlockUuidCount: 500,
      derivedBlockUuidCount: 12,
      warningCount: 1,
      blockingDiagnosticCount: 0,
      diagnosticCount: 1,
      diagnosticsTruncated: false,
      diagnostics: [
        {
          severity: "warning",
          code: "unresolved_page_reference",
          relativePath: "pages/roadmap.md",
          range: null,
          message: "A page reference could not be resolved.",
          remediation: "Create the page before importing.",
        },
      ],
    },
  };
}

const callbacks = {
  hasMoreDiagnostics: false,
  onChoose: () => undefined,
  onCommit: () => undefined,
  onClose: () => undefined,
  onOpenResult: () => undefined,
  onLoadMoreDiagnostics: () => undefined,
};

describe("Logseq import settings UI", () => {
  it("hides the whole subsection when the capability is unavailable", () => {
    const html = renderToStaticMarkup(
      <LogseqImportSectionPresentation
        availability={{ status: "unavailable", reason: "mobile_platform" }}
        state={{ phase: "idle", runId: 0, error: null }}
        {...callbacks}
      />,
    );
    expect(html).toBe("");
  });

  it("renders report counts and disables commit for blockers", () => {
    const prepared = preview();
    const state: LogseqImportState = {
      phase: "preview",
      runId: 1,
      preview: prepared,
      progress: null,
      diagnostics: prepared.report.diagnostics,
      diagnosticsTotal: prepared.report.diagnosticCount,
      diagnosticsLoading: false,
      discarding: false,
      error: null,
    };
    const html = renderToStaticMarkup(<LogseqImportSectionView state={state} {...callbacks} />);

    expect(html).toContain("42");
    expect(html).toContain("512");
    expect(html).toContain("64");
    expect(html).toContain("14");
    expect(html).toContain("3 Excalidraw drawings preserved as source");
    expect(html).toContain("No matching conversion publication is available");
    expect(html).toContain("Logseq import is only available for an empty local workspace.");
    expect(html).toContain("A page reference could not be resolved.");
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>.*Import graph/s);
  });

  it("distinguishes prepared drawing artifacts from preserved source references", () => {
    const prepared = preview();
    prepared.report.drawingConversionState = "prepared";
    prepared.report.preparedExcalidrawCount = 2;
    prepared.report.preservedExcalidrawCount = 1;
    const state: LogseqImportState = {
      phase: "preview",
      runId: 2,
      preview: prepared,
      progress: null,
      diagnostics: prepared.report.diagnostics,
      diagnosticsTotal: prepared.report.diagnosticCount,
      diagnosticsLoading: false,
      discarding: false,
      error: null,
    };

    const html = renderToStaticMarkup(<LogseqImportSectionView state={state} {...callbacks} />);
    expect(html).toContain("2 Excalidraw drawings prepared as PNG");
    expect(html).toContain("1 drawing reference will keep the original source syntax");
    expect(html).toContain("verified again during commit");
  });

  it("offers to open the imported page from the receipt", () => {
    const html = renderToStaticMarkup(
      <LogseqImportSectionView
        state={{
          phase: "result",
          runId: 2,
          error: null,
          result: {
            status: "applied",
            receiptUuid: "019f0000-0000-7000-8000-000000000002",
            pageCount: 42,
            blockCount: 512,
            aliasCount: 4,
            attachmentCount: 10,
            operationCount: 800,
            openPageUuid: "019f0000-0000-7000-8000-000000000003",
          },
        }}
        {...callbacks}
      />,
    );

    expect(html).toContain("Logseq graph imported");
    expect(html).toContain("Open imported notes");
  });
});
