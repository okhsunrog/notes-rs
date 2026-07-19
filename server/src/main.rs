use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

const DEFAULT_EMBEDDING_USD_PER_MILLION_TOKENS: f64 = 0.01;

#[derive(Parser)]
#[command(version, about)]
struct Arguments {
    #[arg(long, default_value = "/etc/notes-rs/config.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Compose embedding inputs without calling an embedding provider.
    InspectIndex {
        /// Path to a notes.db replica.
        #[arg(long)]
        db: PathBuf,
        /// Print the exact composed input and chunks for one page or block.
        #[arg(long)]
        uuid: Option<uuid::Uuid>,
        /// Estimated embedding input price in USD per million tokens.
        #[arg(
            long,
            default_value_t = DEFAULT_EMBEDDING_USD_PER_MILLION_TOKENS
        )]
        cost_per_million_tokens: f64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    if let Some(Command::InspectIndex {
        db,
        uuid,
        cost_per_million_tokens,
    }) = arguments.command
    {
        return inspect_index(db, uuid, cost_per_million_tokens).await;
    }
    let config = notes_server::ServerConfig::load(&arguments.config)?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(&config.log_filter)?)
        .init();
    let state = notes_server::build_state(&config).await?;
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding notes-server to {}", config.listen))?;
    tracing::info!(listen = %config.listen, "notes-server ready");
    let shutdown = state.shutdown.clone();
    axum::serve(listener, notes_server::router(state))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown.cancel();
        })
        .await
        .context("serving notes API")?;
    Ok(())
}

async fn inspect_index(
    db: PathBuf,
    uuid: Option<uuid::Uuid>,
    cost_per_million_tokens: f64,
) -> Result<()> {
    if !cost_per_million_tokens.is_finite() || cost_per_million_tokens < 0.0 {
        bail!("--cost-per-million-tokens must be a finite non-negative number");
    }
    let metadata = std::fs::metadata(&db)
        .with_context(|| format!("reading notes database metadata {}", db.display()))?;
    if !metadata.is_file() {
        bail!("notes database path is not a file: {}", db.display());
    }
    let notes = notes_core::db::open(&db)
        .await
        .with_context(|| format!("opening notes database {}", db.display()))?;
    let report = notes_ai::store::inspect_index(&notes, uuid).await?;
    if let Some(uuid) = uuid
        && report.selected.is_none()
    {
        bail!("UUID {uuid} is not an indexable page or block");
    }
    print!("{}", format_inspection(&report, cost_per_million_tokens));
    Ok(())
}

fn format_inspection(
    report: &notes_ai::store::IndexInspection,
    cost_per_million_tokens: f64,
) -> String {
    use std::fmt::Write;

    let estimated_cost = report.estimated_tokens as f64 * cost_per_million_tokens / 1_000_000.0;
    let mut output = String::new();
    writeln!(output, "documents: {}", report.document_count).unwrap();
    writeln!(
        output,
        "skipped content-free blocks: {}",
        report.skipped_content_free_blocks
    )
    .unwrap();
    writeln!(output, "tier-2 blocks: {}", report.tier_two_blocks).unwrap();
    writeln!(output, "composed characters: {}", report.composed_chars).unwrap();
    writeln!(
        output,
        "estimated input tokens (~4 chars/token): {}",
        report.estimated_tokens
    )
    .unwrap();
    writeln!(
        output,
        "estimated embedding cost (@ {:.4} USD/1M tokens): {:.6} USD",
        cost_per_million_tokens, estimated_cost
    )
    .unwrap();
    writeln!(output, "length histogram (composed chars):").unwrap();
    for bucket in &report.histogram {
        writeln!(output, "  {:>10}: {}", bucket.label, bucket.count).unwrap();
    }
    writeln!(output, "top {} longest:", report.longest.len()).unwrap();
    for item in &report.longest {
        writeln!(
            output,
            "  {}  {:>5}  {} chars",
            content_kind(item.kind),
            item.content_uuid,
            item.composed_chars
        )
        .unwrap();
    }
    if let Some(selected) = &report.selected {
        writeln!(
            output,
            "\nselected {} {} ({} composed chars)",
            content_kind(selected.item.kind),
            selected.item.content_uuid,
            selected.item.composed_chars
        )
        .unwrap();
        writeln!(output, "--- composed text ---").unwrap();
        writeln!(output, "{}", selected.composed_text).unwrap();
        writeln!(
            output,
            "--- {} provider chunk(s) ---",
            selected.chunks.len()
        )
        .unwrap();
        for chunk in &selected.chunks {
            writeln!(
                output,
                "[chunk {}: {} chars]",
                chunk.chunk_index,
                chunk.text.chars().count()
            )
            .unwrap();
            writeln!(output, "{}", chunk.text).unwrap();
        }
    }
    output
}

fn content_kind(kind: notes_ai::store::IndexedContentKind) -> &'static str {
    match kind {
        notes_ai::store::IndexedContentKind::Page => "page",
        notes_ai::store::IndexedContentKind::Block => "block",
    }
}

async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notes_ai::chunking::TextChunk;
    use notes_ai::store::{
        IndexInspection, IndexInspectionItem, IndexLengthBucket, IndexedContentKind,
        InspectedDocument,
    };

    #[test]
    fn inspect_index_subcommand_does_not_require_server_config() {
        let arguments = Arguments::try_parse_from([
            "notes-server",
            "inspect-index",
            "--db",
            "/tmp/notes.db",
            "--uuid",
            "00000000-0000-0000-0000-000000000001",
        ])
        .expect("inspect arguments");

        assert!(matches!(
            arguments.command,
            Some(Command::InspectIndex {
                uuid: Some(_),
                cost_per_million_tokens,
                ..
            }) if cost_per_million_tokens == DEFAULT_EMBEDDING_USD_PER_MILLION_TOKENS
        ));
    }

    #[test]
    fn inspection_output_includes_cost_histogram_and_selected_chunks() {
        let uuid = uuid::Uuid::from_u128(1);
        let item = IndexInspectionItem {
            content_uuid: uuid,
            kind: IndexedContentKind::Block,
            composed_chars: 12,
        };
        let report = IndexInspection {
            document_count: 1,
            skipped_content_free_blocks: 2,
            tier_two_blocks: 0,
            composed_chars: 12,
            estimated_tokens: 3,
            histogram: vec![IndexLengthBucket {
                label: "0-255",
                count: 1,
            }],
            longest: vec![item.clone()],
            selected: Some(InspectedDocument {
                item,
                composed_text: "Page\ncontent".into(),
                chunks: vec![TextChunk {
                    chunk_index: 0,
                    text: "Page\ncontent".into(),
                }],
            }),
        };

        let output = format_inspection(&report, 0.01);
        assert!(output.contains("estimated embedding cost"));
        assert!(output.contains("0-255"));
        assert!(output.contains("--- composed text ---\nPage\ncontent"));
        assert!(output.contains("[chunk 0: 12 chars]"));
    }
}
