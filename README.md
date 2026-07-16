# notes-rs

A graph-native personal knowledge app built with Tauri, React, TypeScript, SQLite/FTS5, and vector search. Notes are stored as an outline of addressable blocks with wikilinks, block references, backlinks, hybrid retrieval, and an OpenRouter-backed agent.

## Development

Install the [Vite+ CLI](https://viteplus.dev/guide/), then let it provision the pinned Node.js and Bun versions and install dependencies:

```sh
vp install
vp dev
```

Run the desktop app in development mode with:

```sh
vp run tauri dev
```

### MCP UI inspection

Debug builds include the localhost-only [MCP Server Tauri](https://github.com/hypothesi/mcp-server-tauri) bridge. The repository's `.codex/config.toml` registers its pinned MCP server with Codex; trust the project and restart Codex after the first checkout so the project-scoped server is loaded.

With the Tauri development app running, Codex can capture screenshots and DOM snapshots, inspect logs and IPC traffic, find elements, click, type, scroll, resize windows, and execute JavaScript in the webview. A pinned local CLI is also available for terminal diagnostics:

```sh
vp exec tauri-mcp driver-session start --port 9223
vp exec tauri-mcp webview-dom-snapshot
vp exec tauri-mcp webview-screenshot --file screenshot.png
```

The bridge is not registered in release builds and binds to `127.0.0.1` in debug builds.

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

## Configuration

Desktop launches load `.env` from the platform-specific Tauri app-data directory. The default cloud setup requires:

```dotenv
OPENROUTER_API_KEY=your-key
```

Optional embedding settings include `EMBED_PROVIDER`, `EMBED_MODEL`, and `EMBED_NDIMS`; reranking can be configured with `RERANK_PROVIDER` and `RERANK_MODEL`. The supported default path uses OpenRouter. Offline embedding and reranking are available in builds made with the Rust `local-models` feature.
