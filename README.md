# notes-rs

notes-rs is a local-first personal knowledge app for desktop and Android. Typed pages contain addressable outline or document blocks connected by wikilinks, block references, and backlinks. React and Tauri provide the shared client; Rust, SQLite, and FTS5 keep writing and lexical search available offline; an optional self-hosted Axum server provides realtime sync and all AI functionality.

## What works

- Hierarchical block editing with autosave, Markdown rendering, wikilink and block-reference autocomplete, device-local remembered folding, paragraph splitting, keyboard navigation, and action-based undo/redo.
- Local pages, graph, backlinks, attachments, import/export, timestamped recovery backups, and English/Russian FTS5 search. These features work without a server.
- Near-realtime op-based sync over HTTP and WebSocket, with an offline outbox, HLC/LWW conflict resolution, tombstones, deterministic structural reconciliation, snapshot bootstrap, and content-addressed blobs.
- Server-owned semantic retrieval, reranking, entity extraction, and streaming chat. Extracted entities remain disposable server-side AI data rather than client graph nodes; the clients contain no provider keys, vector database, embedding model, or AI background workers.
- Remote AI administration in Settings: provider URLs and models, write-only API keys, capability probes, automatic-indexing and extraction switches, queue progress, failures, and reindex controls.
- A full-workspace graph view, six color palettes with light/dark/system brightness, responsive mobile navigation, Android edge-to-edge safe areas, and configurable native or client window decorations on desktop.
- Debug-only, localhost-bound Tauri MCP integration for live screenshots, accessibility snapshots, input, logs, and IPC inspection.

Select **New note** or press `Ctrl/Cmd+N` to start writing. Enter a title, then press `Enter` to focus the first block. `Ctrl/Cmd+K` opens search. The graph has its own full-workspace surface; the side companion is reserved for AI.

### Outliner shortcuts

| Shortcut                          | Action                                                       |
| --------------------------------- | ------------------------------------------------------------ |
| `Enter`                           | Save and create the next sibling block                       |
| `Shift+Enter`                     | Insert a newline inside the current block                    |
| `Tab` / `Shift+Tab`               | Indent / outdent                                             |
| `Ctrl/Cmd+Up` / `Ctrl/Cmd+Down`   | Reorder among siblings                                       |
| `Ctrl/Cmd+Enter`                  | Toggle the persisted fold state                              |
| `Backspace` on an empty leaf      | Delete the block                                             |
| `Ctrl/Cmd+Z` / `Ctrl/Cmd+Shift+Z` | Text undo/redo while editing; structural undo/redo elsewhere |

## Architecture

The durable source of truth is a UUID-addressed operation stream. Each client and the server materialize that stream into their own `notes.db`; integer SQLite IDs never cross replica boundaries. Local mutations and remote operations pass through the same idempotent apply engine. Per-field HLC clocks preserve independent title, content, and structure edits, while the sync client batches remote apply and cursor advancement atomically.

Persisted Rust state is exposed through generated tauri-specta bindings and cached in TanStack Query. One typed domain-event adapter invalidates the narrow affected query keys. Draft text and caret state stay local to the editor instead of being mixed into the backend cache.

The server keeps source pages, blocks, attachments, and the oplog in SQLite. Derived embeddings, generations, indexing jobs, extracted entities, and extraction state live in a separate disposable `ai.db` behind a `VectorStore` interface; they are not synced into the client graph. sqlite-vec is loaded only by the server binary. Provider settings are bootstrapped from the server TOML on first start and subsequently managed from the authenticated application Settings page; provider secrets are stored server-side with owner-only permissions and are never returned to the webview.

The detailed implementation record is in [SYNC_ARCHITECTURE_PLAN.md](SYNC_ARCHITECTURE_PLAN.md).
The accepted Outline/Document, Live Preview, Reading/Split, and CodeMirror boundary is recorded in
[EDITOR_ARCHITECTURE.md](EDITOR_ARCHITECTURE.md).

## Development

Install the [Vite+ CLI](https://viteplus.dev/guide/), then let it provision the pinned Node.js and Bun versions:

```sh
vp install
vp dev
```

Run the desktop client:

```sh
vp run desktop:dev
```

Initialize and build Android from the same client source:

```sh
vp run tauri android init
vp run tauri android build --debug
```

The only build-time environment input is Tauri's `TAURI_DEV_HOST`, which Vite needs for Android/device hot reload. Product configuration does not read process environment variables.

### Validation

```sh
vp check
vp test
vp build
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked --all-features
```

CI also regenerates the tauri-specta TypeScript bindings and fails when the committed contract has drifted from Rust.

### Live UI inspection

Debug desktop builds include [MCP Server Tauri](https://github.com/hypothesi/mcp-server-tauri). The project-scoped `.codex/config.toml` pins its server. With the development app running, the bridge can inspect the accessibility tree, capture screenshots, click, type, resize, and inspect logs and Tauri IPC.

```sh
vp exec tauri-mcp driver-session start --port 9223
vp exec tauri-mcp webview-dom-snapshot --type accessibility
vp exec tauri-mcp webview-screenshot --file screenshot.png
```

The MCP bridge and relaxed development CSP are absent from release builds.

## Configuration and deployment

Client configuration lives only in the typed application Settings file: appearance, window frame, server URL, bearer token, and sync enablement. There are no `.env` files or alternate client configuration paths. If no server is configured, editing, graph navigation, attachments, history, and FTS remain available; sync, semantic search, extraction, and chat are visibly unavailable.

The server uses `server/config.example.toml` as a first-start bootstrap. After that, provider configuration and runtime indexing controls are changed remotely from Settings. OpenRouter is the current embeddings/reranking deployment, while completion can use any OpenAI Chat Completions-compatible or Anthropic Messages-compatible endpoint with a custom base URL.

Build a static musl server archive or deploy it through the sibling `cloud-forge` Ansible project:

```sh
just package-server
just deploy-server
```

TLS and public routing belong to the existing reverse proxy; the notes server binds privately and ships as one static binary plus its bootstrap TOML.

## Remaining work

The core architecture is implemented, but this is still a pre-release project. The main remaining work is:

- cursor-aware oplog compaction and device registration/pairing; snapshots are created and bootstrap works, but the server deliberately does not delete history until it can prove every registered device is past the compaction floor;
- uninterrupted semantic queries while a provider change builds a new vector generation; generation activation is atomic, but the new provider runtime currently becomes authoritative while its generation is building;
- scoped and revocable per-device credentials instead of the current static token mapping;
- Markdown-vault, Obsidian, and Logseq import, plus daily notes, templates, properties, saved queries, and an extension model;
- drag-and-drop block movement, cross-block selection, transclusion, richer Markdown authoring, graph filters/layouts, and larger-corpus performance work;
- signed production packages and end-to-end UI/accessibility regression coverage on desktop and real Android hardware.
