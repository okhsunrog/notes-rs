# Tangleaf documentation

## Architecture

- [Sync, storage, and AI](architecture/sync-storage-ai.md)
- [Editor and page presentation](architecture/editor.md)
- [Workspace panes](architecture/workspace.md)
- [Journals and Logseq import](architecture/journals.md)

These design records retain their original dates and implementation boundaries.
Source-code paths in them are relative to the repository root. Consult current
code and tests when an older implementation-status statement matters.

## Planning

- [Roadmap](planning/roadmap.md): implementation stages and development history.
- [Backlog](planning/backlog.md): deferred ideas and review findings to re-check
  before scheduling work.

## Historical records

Earlier task plans and measurement results are retained for technical context,
not as instructions to execute against the current codebase:

- [Search and correctness](history/search-and-fixes.md)
- [Embedding pipeline](history/embeddings.md)
- [Markdown import and export](history/markdown-import-export.md)
- [Image-preview performance](history/image-preview-performance.md)
- [Page-opening responsiveness](history/page-opening-performance.md)

Keep product-facing setup and usage information in the root README. Put durable
design documentation here; do not add session handoffs or temporary merge reports
to the repository root. Completed one-off reports can remain in Git history.
