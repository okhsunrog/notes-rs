# notes-rs

A graph-native personal knowledge app built with Tauri, React, TypeScript, SQLite/FTS5, and vector search. Notes are stored as an outline of addressable blocks with wikilinks, block references, backlinks, hybrid retrieval, and an OpenRouter-backed agent.

The right-hand Graph tab visualizes the selected page's local knowledge neighborhood and backlinks. Pages can contain file attachments copied into app data and embedded in portable archives. Settings includes JSON export/import, timestamped local backups, and explicit push/pull folder sync suitable for Syncthing, Nextcloud, or similar tools; pull, import, attachment deletion, and page deletion create a recovery backup automatically.

## Features

- Hierarchical block outliner with autosave, wikilinks, block references, autocomplete, persisted folding, structural undo/redo, atomic paragraph splitting, attachments, and keyboard navigation.
- SQLite/FTS5 search with English/Russian stemming, `sqlite-vec` semantic retrieval, reciprocal-rank fusion, backlink boost, configurable reranking, and an OpenRouter-backed graph agent.
- Background embeddings and entity extraction with exponential backoff, stale-result protection, queue status, pause/resume, retry, and cancellation controls.
- Local graph/backlinks explorer, six configurable color atmospheres with tuned light/dark variants, responsive narrow-window tabs, import/export, backups, and manual conflict-safe folder sync.
- Debug-only, localhost-bound Tauri MCP bridge for accessibility snapshots, screenshots, interaction, logs, and IPC inspection.

### Outliner shortcuts

| Shortcut                          | Action                                                              |
| --------------------------------- | ------------------------------------------------------------------- |
| `Enter`                           | Save and create the next sibling block                              |
| `Shift+Enter`                     | Insert a newline inside the current block                           |
| `Tab` / `Shift+Tab`               | Indent / outdent                                                    |
| `Ctrl/Cmd+↑` / `Ctrl/Cmd+↓`       | Reorder among siblings                                              |
| `Ctrl/Cmd+Enter`                  | Toggle the persisted fold state                                     |
| `Backspace` on an empty leaf      | Delete the block                                                    |
| `Ctrl/Cmd+Z` / `Ctrl/Cmd+Shift+Z` | Native text undo/redo while editing; structural undo/redo elsewhere |

### Start writing

Select **New note** in the sidebar or press `Ctrl/Cmd+N`. A uniquely named untitled page is created and its title is selected immediately. Type a title and press `Enter` to move straight into the first block. `Ctrl/Cmd+K` opens search from anywhere in the workspace.

Appearance is configured independently in **Settings → Appearance**:

- Brightness: System, Light, or Dark.
- Palette: Iris, Tidal, Ember, Sakura, Nordic, or Moss.

Palette changes are applied immediately and kept locally for the next launch.

## Current scope

This repository is a capable demo rather than a complete Obsidian or Logseq replacement. The main remaining product work is:

- Markdown vault and importer interoperability, including migration from Obsidian and Logseq.
- Daily notes, templates, tags/properties UI, saved queries, and a plugin or extension model.
- Automatic multi-device sync with conflict resolution; current folder sync exchanges explicit snapshots.
- Drag-and-drop block movement, cross-block selection, inline transclusion, and richer Markdown editing.
- A scalable interactive graph with filters and layouts; the current graph is a compact local-neighborhood view.
- End-to-end UI regression tests in CI, accessibility testing beyond semantic snapshots, and production packaging/signing across all supported platforms.
- Bundle splitting and lazy loading for heavier Settings, graph, and assistant surfaces.

## Development

Install the [Vite+ CLI](https://viteplus.dev/guide/), then let it provision the pinned Node.js and Bun versions and install dependencies:

```sh
vp install
vp dev
```

Run the desktop app in development mode with:

```sh
vp run desktop:dev
```

### MCP UI inspection

Debug builds include the localhost-only [MCP Server Tauri](https://github.com/hypothesi/mcp-server-tauri) bridge. The repository's `.codex/config.toml` registers its pinned MCP server with Codex; trust the project and restart Codex after the first checkout so the project-scoped server is loaded.

With the Tauri development app running, Codex can capture screenshots and DOM snapshots, inspect logs and IPC traffic, find elements, click, type, scroll, resize windows, and execute JavaScript in the webview. A pinned local CLI is also available for terminal diagnostics:

```sh
vp exec tauri-mcp driver-session start --port 9223
vp exec tauri-mcp webview-dom-snapshot --type accessibility
vp exec tauri-mcp webview-screenshot --file screenshot.png
```

The development command applies `src-tauri/tauri.dev.conf.json`, which enables the global Tauri API required by the bridge and permits the Vite development server in the CSP. The bridge is not registered in release builds and binds to `127.0.0.1` in debug builds.

The standard validation workflow is:

```sh
vp check
vp test
vp build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Vite+ owns frontend formatting, linting, type checking, testing, dependency management, runtime selection, and Git hooks. Its project configuration is centralized in `vite.config.ts`.

Release webviews use a restrictive Content Security Policy and do not expose the global Tauri API. Both are relaxed only by the explicit development overlay used for MCP inspection and hot reload.

## Configuration

Open **Settings** from the title bar to configure embedding and reranking providers, API keys, model dimensions, and window decorations. Linux offers the native GTK header, a borderless notes-rs frame, and native-Wayland KWin server decorations. The KWin mode removes Tao's custom GTK header and lets GTK negotiate the KDE server-decoration protocol; it does not use XWayland. Secrets are stored in the platform-specific Tauri app-data `.env` with owner-only permissions and existing values are never returned to the webview. Provider and window-backend changes take effect after using **Restart app**.

The same file can be managed manually. The default cloud setup requires:

```dotenv
OPENROUTER_API_KEY=your-key
```

Optional embedding settings include `EMBED_PROVIDER`, `EMBED_MODEL`, and `EMBED_NDIMS`; reranking can be configured with `RERANK_PROVIDER` and `RERANK_MODEL`. The supported default path uses OpenRouter. Offline embedding and reranking are available in builds made with the Rust `local-models` feature.
