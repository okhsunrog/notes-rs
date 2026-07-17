# notes-rs legacy Excalidraw converter

This is an isolated, one-time migration tool for legacy Excalidraw `0.12.0`
documents. It is not part of the notes-rs runtime and must not be bundled into
the desktop, Android, or server applications.

The converter restores a drawing with Excalidraw's official `restore` API,
filters it through `getNonDeletedElements`, and renders it through
`exportToBlob`. The result is a white/source-background PNG at up to 2x scale,
bounded to 8192 pixels on either axis and 25 megapixels. Browser network access
is denied; only data/blob assets embedded in the source document can render.

## Install and run

The dependency graph and lockfile are intentionally local to this directory:

```sh
cargo run -p notes-import --example drawing_conversion_inputs -- /path/to/logseq-graph

cd tools/excalidraw-converter
bun install --frozen-lockfile
bunx playwright install chromium
mkdir -p /path/outside/the/graph
bun run convert -- \
  --source-root /path/to/logseq-graph \
  --output /path/outside/the/graph/excalidraw-publication \
  --source-manifest-sha256 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef \
  --drawing draws/diagram.excalidraw \
  --drawing draws/another.excalidraw
```

The Rust helper prints the exact source-manifest digest and the drawing
allowlist that belong to one immutable scan. Pass those values to the
converter unchanged. For notes-rs' automatic import discovery, publish to:

```text
<app-data>/logseq-drawing-conversions/<source-manifest-sha256>/
```

`--drawing` is an explicit allowlist and may be repeated. Every entry must come
from the prepared immutable source manifest. The helper lists all manifest
drawings, including unreferenced or empty drawings, so the publication remains
an auditable one-time conversion run. The importer consumes only exact
references from note content and ignores other manifest-bound outputs. The
converter never scans or writes the graph itself.

The output is published atomically as one new directory:

```text
excalidraw-publication/
  conversion-bundle.json
  artifacts/<prefix>/<source-path-sha256>-<png-sha256>.png
```

The output directory must not already exist, its parent directory must already
exist, and it must be disjoint from the source root. The JSON bundle contains
only relative paths, hashes, sizes, dimensions, versions, and status/error
codes. It never contains absolute paths or drawing/note text. There are no
timestamps, so identical inputs rendered by the same pinned browser produce
byte-identical bundles and PNGs.

Each drawing has exactly one of the closed statuses `converted`,
`skipped_empty`, or `failed`. Failures use a closed v1 error-code enum rather
than arbitrary provider/browser text. A bundle with any failed drawing is
still published for audit, while the CLI exits non-zero so the import cannot
silently continue.

To validate the tool:

```sh
bun run typecheck
bun test
```

The fixtures are synthetic and sanitized. Tests never inspect or modify the
real Logseq graph.
