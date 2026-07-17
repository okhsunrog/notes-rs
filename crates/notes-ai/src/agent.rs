use crate::embed::{EmbedderBackend, RerankBackend};
use anyhow::Context;
use futures::StreamExt;
use llm_relay::RigClient;
use notes_core::db::{self, Node, SearchHit};
use notes_core::{Connection, NodeKind};
use rig::agent::MultiTurnStreamItem;
use rig::client::CompletionClient;
use rig::completion::{CompletionModel, Message, Prompt};
use rig::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingPrompt};
use rig::tool::{Tool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const SYSTEM_PROMPT: &str = r#"
You are an assistant embedded in a personal knowledge graph (notes-rs).
The user's notes are stored as nodes (blocks, pages, entities, tags) connected by typed edges.
You answer questions by retrieving from the graph using tools — never invent facts.

Workflow:
1. Use `list_pages` for inventory questions such as "what notes do I have?",
   "list all notes", or requests for a notebook-wide overview. Then use
   `read_subtree` on the returned page ids when their contents are needed.
2. Use `search_and_expand` for relevance questions: it does hybrid retrieval, then
   walks the graph one hop from each seed and reranks the merged set. This is
   the default because the graph almost always adds useful context.
3. Use `search_agentic` only when you want plain text-relevance with no graph
   expansion (e.g. you're looking for exact wording).
4. Drill into specific nodes once you have ids:
   - `read_ancestors` for the outline breadcrumb above a block,
   - `read_subtree` to read everything under a page or section,
   - `find_backlinks` for "who points at this?",
   - `find_tagged` for "what mentions entity/tag X?",
   - `neighbors` for undirected graph walks,
   - `get_node` for a single row.
5. Cite node IDs (e.g. "see node #42") in your final answer.
6. If nothing relevant found, say so plainly. Do not fabricate.

When calling a search tool, always write a fully self-contained query that resolves any references
from the chat so far ("that one", "the second", "те", etc.) — the search index does not see the
conversation history, only your query string. Prefer concrete nouns over pronouns.
"#;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct AgentError(String);

impl From<anyhow::Error> for AgentError {
    fn from(e: anyhow::Error) -> Self {
        AgentError(format!("{e:#}"))
    }
}

fn into_tool_err(e: anyhow::Error) -> ToolError {
    ToolError::ToolCallError(format!("{e:#}").into())
}

// ───────────────────────── query rewriter ─────────────────────────

/// Resolves conversational references in a tool-issued search query.
///
/// The agent already sees the chat history when it picks tool args, but
/// cheaper models sometimes echo the user's literal phrasing ("and the
/// second one?") as the search query. This is a safety net: when a query
/// looks contextual we issue one extra LLM call to rewrite it into a
/// standalone form. When history is empty or the query already looks
/// self-contained we pass through.
#[derive(Clone)]
pub struct QueryRewriter {
    /// Pre-formatted history block, ready to drop into a prompt.
    history_text: Arc<String>,
    enabled: bool,
    config: Option<llm_relay::ClientConfig>,
}

impl QueryRewriter {
    pub fn new(history: &[ChatTurn], enabled: bool, config: llm_relay::ClientConfig) -> Self {
        use std::fmt::Write;
        let mut s = String::new();
        for t in history {
            match t {
                ChatTurn::User { text } => {
                    let _ = writeln!(s, "[user]: {text}");
                }
                ChatTurn::Assistant { text } => {
                    let _ = writeln!(s, "[assistant]: {text}");
                }
            }
        }
        Self {
            history_text: Arc::new(s),
            enabled,
            config: Some(config),
        }
    }

    pub fn empty() -> Self {
        Self {
            history_text: Arc::new(String::new()),
            enabled: false,
            config: None,
        }
    }

    pub async fn rewrite(&self, query: &str) -> String {
        if !self.enabled || self.history_text.is_empty() || !looks_contextual(query) {
            return query.to_string();
        }
        match self.try_rewrite(query).await {
            Ok(s) if !s.is_empty() => s,
            Ok(_) => query.to_string(),
            Err(e) => {
                tracing::warn!(error = ?e, query, "query rewrite failed; using original");
                query.to_string()
            }
        }
    }

    async fn try_rewrite(&self, query: &str) -> anyhow::Result<String> {
        let prompt = format!(
            "Conversation history:\n{history}\nSearch query: {query}\n\n\
             Rewrite the query into a fully standalone form that resolves \
             any conversational references (pronouns, 'the second one', \
             'this', 'that one', 'те', 'этот', 'предыдущий', etc.) using \
             the history above. Keep it concise. If the query already \
             stands alone, return it unchanged. Reply with ONLY the \
             rewritten query — no quotes, no explanation, no labels.",
            history = self.history_text,
            query = query,
        );
        let config = self
            .config
            .clone()
            .context("query rewriting client is not configured")?;
        match config.rig_client()? {
            RigClient::OpenAi(client) => {
                rewrite_with_model(client.completion_model(&config.model), prompt).await
            }
            RigClient::Anthropic(client) => {
                rewrite_with_model(client.completion_model(&config.model), prompt).await
            }
        }
    }
}

async fn rewrite_with_model<M: CompletionModel + 'static>(
    model: M,
    prompt: String,
) -> anyhow::Result<String> {
    let agent = rig::agent::AgentBuilder::new(model)
        .preamble("You rewrite search queries to be self-contained.")
        .max_tokens(120)
        .build();
    let raw: String = agent.prompt(prompt).await?;
    Ok(raw
        .trim()
        .trim_start_matches("Rewritten query:")
        .trim_matches('"')
        .trim_matches('`')
        .trim()
        .to_string())
}

/// Heuristic gate: only call the rewriter when the query has tells of a
/// conversational reference. Avoids the extra LLM call on the vast majority
/// of obviously-standalone queries.
fn looks_contextual(q: &str) -> bool {
    if q.trim().is_empty() {
        return false;
    }
    if q.split_whitespace().count() <= 3 {
        return true;
    }
    let lower = q.to_lowercase();
    // English + Russian demonstratives/pronouns commonly used to refer back.
    const CUES: &[&str] = &[
        " this",
        " that",
        " it ",
        " these",
        " those",
        " same",
        " above",
        " previous",
        " former",
        " latter",
        " second",
        " first",
        " third",
        "тот",
        "то ",
        "эт",
        "ту ",
        "те ",
        "предыдущ",
        "выше",
        "ранее",
    ];
    let padded = format!(" {lower} ");
    CUES.iter().any(|c| padded.contains(c))
}

// ───────────────────────── search_agentic ─────────────────────────

#[derive(Clone)]
pub struct SearchAgentic {
    pub conn: Connection,
    pub embedder: Arc<dyn EmbedderBackend>,
    pub reranker: Arc<dyn RerankBackend>,
    pub rewriter: QueryRewriter,
}

#[derive(Deserialize)]
pub struct SearchArgs {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
}
fn default_limit() -> u32 {
    8
}

const MAX_SEARCH_LIMIT: u32 = 32;
const MAX_QUERY_CHARS: usize = 4_096;

fn validate_search_args(args: &SearchArgs) -> Result<(), ToolError> {
    if args.query.trim().is_empty() || args.query.chars().count() > MAX_QUERY_CHARS {
        return Err(ToolError::ToolCallError(
            format!("query must contain 1 to {MAX_QUERY_CHARS} characters").into(),
        ));
    }
    if args.limit == 0 || args.limit > MAX_SEARCH_LIMIT {
        return Err(ToolError::ToolCallError(
            format!("limit must be between 1 and {MAX_SEARCH_LIMIT}").into(),
        ));
    }
    Ok(())
}

impl Tool for SearchAgentic {
    const NAME: &'static str = "search_agentic";
    type Error = ToolError;
    type Args = SearchArgs;
    type Output = Vec<SearchHit>;

    fn description(&self) -> String {
        "Hybrid search (BM25 + semantic + BGE rerank) over notes. Returns ranked nodes with scores. Prefer this for most queries.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": MAX_QUERY_CHARS, "description": "Search query in the user's language" },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_SEARCH_LIMIT, "description": "Max results to return (default 8)", "default": 8 }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        validate_search_args(&args)?;
        let query = self.rewriter.rewrite(&args.query).await;
        let emb = self
            .embedder
            .embed_query(query.clone())
            .await
            .map_err(into_tool_err)?;
        // Pool floor: at low `limit` (e.g. 3) the default `limit*4 = 12` is
        // too narrow when BM25 and vector channels disagree. Widen the rerank
        // input so we don't starve the reranker of plausible candidates.
        let pool = args.limit.saturating_mul(4).max(RERANK_POOL_MIN);
        let candidates = db::search_hybrid(&self.conn, query.clone(), emb, pool)
            .await
            .map_err(into_tool_err)?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let docs: Vec<String> = candidates
            .iter()
            .map(|h| {
                format!(
                    "{}\n{}",
                    h.node.title.as_deref().unwrap_or(""),
                    h.node.content
                )
            })
            .collect();
        let scored = self
            .reranker
            .rerank(query, docs)
            .await
            .map_err(into_tool_err)?;
        // Score floor: BGE-reranker scores below ~RELEVANCE_FLOOR are
        // effectively irrelevant — passing them to the agent invites
        // hallucinate-by-citation. Returning empty when nothing clears the
        // bar gives the model a clean "nothing found" signal.
        Ok(scored
            .into_iter()
            .filter(|(_, s)| *s as f64 >= RELEVANCE_FLOOR)
            .take(args.limit as usize)
            .filter_map(|(idx, score)| {
                candidates.get(idx).map(|candidate| SearchHit {
                    node: candidate.node.clone(),
                    score: score as f64,
                })
            })
            .collect())
    }
}

const RERANK_POOL_MIN: u32 = 32;
const RELEVANCE_FLOOR: f64 = 0.30;
/// Cap on the merged seeds+neighbors pool before reranking. The reranker is
/// the per-query cost bottleneck; ~64 docs is fine, ~512 is sluggish.
const EXPAND_POOL_MAX: usize = 64;
const LOW_CONFIDENCE_FALLBACK_LIMIT: usize = 4;

fn select_reranked_hits(
    scored: Vec<(usize, f32)>,
    candidates: &[Node],
    limit: usize,
) -> Vec<SearchHit> {
    let confident: Vec<SearchHit> = scored
        .iter()
        .filter(|(_, score)| *score as f64 >= RELEVANCE_FLOOR)
        .take(limit)
        .filter_map(|(index, score)| {
            candidates.get(*index).cloned().map(|node| SearchHit {
                node,
                score: *score as f64,
            })
        })
        .collect();
    if !confident.is_empty() {
        return confident;
    }

    // Cross-language and broad inventory-like queries can produce uniformly
    // low reranker scores even when semantic retrieval found the right notes.
    // Returning a small, explicitly low-scored fallback lets the agent inspect
    // real content instead of falsely claiming that the notebook is empty.
    scored
        .into_iter()
        .take(limit.min(LOW_CONFIDENCE_FALLBACK_LIMIT))
        .filter_map(|(index, score)| {
            candidates.get(index).cloned().map(|node| SearchHit {
                node,
                score: score as f64,
            })
        })
        .collect()
}

// ───────────────────────── search_and_expand ─────────────────────────

/// Hybrid search + 1-hop graph expansion + rerank. The default retrieval
/// tool: starts from the same hybrid candidates as `search_agentic`, then
/// pulls each seed's immediate neighbors (refs, mentions, relations) into
/// the candidate set before reranking. This is what makes the typed-edge
/// graph actually do work for the model instead of being decoration.
#[derive(Clone)]
pub struct SearchAndExpand {
    pub conn: Connection,
    pub embedder: Arc<dyn EmbedderBackend>,
    pub reranker: Arc<dyn RerankBackend>,
    pub rewriter: QueryRewriter,
}

impl Tool for SearchAndExpand {
    const NAME: &'static str = "search_and_expand";
    type Error = ToolError;
    type Args = SearchArgs;
    type Output = Vec<SearchHit>;

    fn description(&self) -> String {
        "Hybrid search then walk one graph hop from each seed (refs/mentions/relations) and rerank the merged pool. Prefer this over `search_agentic` for most questions — the extra context usually helps.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": MAX_QUERY_CHARS, "description": "Search query in the user's language" },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_SEARCH_LIMIT, "description": "Max results to return (default 8)", "default": 8 }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        validate_search_args(&args)?;
        let query = self.rewriter.rewrite(&args.query).await;
        let emb = self
            .embedder
            .embed_query(query.clone())
            .await
            .map_err(into_tool_err)?;
        let seed_pool = args.limit.saturating_mul(4).max(RERANK_POOL_MIN);
        let seeds = db::search_hybrid(&self.conn, query.clone(), emb, seed_pool)
            .await
            .map_err(into_tool_err)?;
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        // Walk one hop from each seed; dedup by id, cap the merged pool.
        use std::collections::HashMap;
        let mut pool: HashMap<i64, Node> = HashMap::new();
        for s in &seeds {
            pool.entry(s.node.id).or_insert_with(|| s.node.clone());
        }
        for s in &seeds {
            if pool.len() >= EXPAND_POOL_MAX {
                break;
            }
            let neigh = db::neighbors(&self.conn, s.node.id, 1)
                .await
                .map_err(into_tool_err)?;
            for n in neigh {
                if pool.len() >= EXPAND_POOL_MAX {
                    break;
                }
                pool.entry(n.id).or_insert(n);
            }
        }
        let candidates: Vec<Node> = pool.into_values().collect();
        let docs: Vec<String> = candidates
            .iter()
            .map(|n| format!("{}\n{}", n.title.as_deref().unwrap_or(""), n.content))
            .collect();
        let scored = self
            .reranker
            .rerank(query, docs)
            .await
            .map_err(into_tool_err)?;
        Ok(select_reranked_hits(
            scored,
            &candidates,
            args.limit as usize,
        ))
    }
}

// ───────────────────────── list_pages ─────────────────────────

#[derive(Clone)]
pub struct ListPages {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct ListPagesArgs {
    #[serde(default = "default_page_limit")]
    pub limit: u32,
}

fn default_page_limit() -> u32 {
    100
}

impl Tool for ListPages {
    const NAME: &'static str = "list_pages";
    type Error = ToolError;
    type Args = ListPagesArgs;
    type Output = Vec<Node>;

    fn description(&self) -> String {
        "List notebook pages without semantic filtering. Use for inventory questions such as 'what notes do I have?' before reading page subtrees.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 100 }
            }
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if !(1..=200).contains(&args.limit) {
            return Err(ToolError::ToolCallError(
                "limit must be between 1 and 200".into(),
            ));
        }
        db::list_pages(&self.conn, args.limit)
            .await
            .map_err(into_tool_err)
    }
}

// ───────────────────────── neighbors ─────────────────────────

#[derive(Clone)]
pub struct Neighbors {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct NeighborsArgs {
    pub id: i64,
    #[serde(default = "default_depth")]
    pub depth: u32,
}
fn default_depth() -> u32 {
    1
}
const MAX_GRAPH_DEPTH: u32 = 6;

impl Tool for Neighbors {
    const NAME: &'static str = "neighbors";
    type Error = ToolError;
    type Args = NeighborsArgs;
    type Output = Vec<Node>;

    fn description(&self) -> String {
        "Return nodes reachable from the given node id within `depth` hops over the graph edges (undirected).".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "Source node id" },
                "depth": { "type": "integer", "minimum": 1, "maximum": MAX_GRAPH_DEPTH, "description": "Hop limit (default 1)", "default": 1 }
            },
            "required": ["id"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if args.depth == 0 || args.depth > MAX_GRAPH_DEPTH {
            return Err(ToolError::ToolCallError(
                format!("depth must be between 1 and {MAX_GRAPH_DEPTH}").into(),
            ));
        }
        db::neighbors(&self.conn, args.id, args.depth)
            .await
            .map_err(into_tool_err)
    }
}

// ───────────────────────── find_backlinks ─────────────────────────

#[derive(Clone)]
pub struct FindBacklinks {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct FindBacklinksArgs {
    pub id: i64,
    /// Optional edge-kind filter, e.g. "refs" or "mentions".
    #[serde(default)]
    pub kind: Option<String>,
}

impl Tool for FindBacklinks {
    const NAME: &'static str = "find_backlinks";
    type Error = ToolError;
    type Args = FindBacklinksArgs;
    type Output = Vec<Node>;

    fn description(&self) -> String {
        "Find nodes that link TO the given node (incoming edges only — different from `neighbors`, which is undirected). Optional `kind` filter: 'refs' for wikilinks/block-refs, 'mentions' for entity mentions.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "Target node id" },
                "kind": { "type": ["string", "null"], "description": "Optional edge-kind filter" }
            },
            "required": ["id"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        db::find_backlinks(&self.conn, args.id, args.kind)
            .await
            .map_err(into_tool_err)
    }
}

// ───────────────────────── read_ancestors ─────────────────────────

#[derive(Clone)]
pub struct ReadAncestors {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct ReadAncestorsArgs {
    pub id: i64,
}

impl Tool for ReadAncestors {
    const NAME: &'static str = "read_ancestors";
    type Error = ToolError;
    type Args = ReadAncestorsArgs;
    type Output = Vec<Node>;

    fn description(&self) -> String {
        "Walk the parent chain from this node up to the page root. Returned root-first; the node itself is the last element. Useful for getting an outline breadcrumb / surrounding context for a block.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "integer" } },
            "required": ["id"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        db::read_ancestors(&self.conn, args.id)
            .await
            .map_err(into_tool_err)
    }
}

// ───────────────────────── read_subtree ─────────────────────────

#[derive(Clone)]
pub struct ReadSubtree {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct ReadSubtreeArgs {
    pub id: i64,
    #[serde(default = "default_subtree_depth")]
    pub depth: u32,
}
fn default_subtree_depth() -> u32 {
    4
}
const MAX_SUBTREE_DEPTH: u32 = 12;

impl Tool for ReadSubtree {
    const NAME: &'static str = "read_subtree";
    type Error = ToolError;
    type Args = ReadSubtreeArgs;
    type Output = Vec<Node>;

    fn description(&self) -> String {
        "Return all descendants of `id` up to `depth` levels, in outline (DFS pre-order) order. Use this to read the entire subtree below a page or section.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer" },
                "depth": { "type": "integer", "minimum": 1, "maximum": MAX_SUBTREE_DEPTH, "description": "Max levels to descend (default 4)", "default": 4 }
            },
            "required": ["id"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if args.depth == 0 || args.depth > MAX_SUBTREE_DEPTH {
            return Err(ToolError::ToolCallError(
                format!("depth must be between 1 and {MAX_SUBTREE_DEPTH}").into(),
            ));
        }
        db::read_subtree(&self.conn, args.id, args.depth)
            .await
            .map_err(into_tool_err)
    }
}

// ───────────────────────── find_tagged ─────────────────────────

#[derive(Clone)]
pub struct FindTagged {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct FindTaggedArgs {
    pub title: String,
}

impl Tool for FindTagged {
    const NAME: &'static str = "find_tagged";
    type Error = ToolError;
    type Args = FindTaggedArgs;
    type Output = Vec<Node>;

    fn description(&self) -> String {
        "Find blocks/pages that mention an entity or tag with the given title (case-insensitive). Use this when the user asks about a specific person/project/concept and you want everything connected to it.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "title": { "type": "string" } },
            "required": ["title"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        db::find_tagged(&self.conn, args.title)
            .await
            .map_err(into_tool_err)
    }
}

// ───────────────────────── get_node ─────────────────────────

#[derive(Clone)]
pub struct GetNode {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct GetNodeArgs {
    pub id: i64,
}

impl Tool for GetNode {
    const NAME: &'static str = "get_node";
    type Error = ToolError;
    type Args = GetNodeArgs;
    type Output = Option<Node>;

    fn description(&self) -> String {
        "Fetch a single node by its integer id.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "integer" } },
            "required": ["id"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        db::get_node(&self.conn, args.id)
            .await
            .map_err(into_tool_err)
    }
}

// ───────────────────────── create_node ─────────────────────────

#[derive(Clone)]
pub struct CreateNode {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct CreateNodeArgs {
    pub kind: NodeKind,
    pub title: Option<String>,
    pub content: String,
}

impl Tool for CreateNode {
    const NAME: &'static str = "create_node";
    type Error = ToolError;
    type Args = CreateNodeArgs;
    type Output = Node;

    fn description(&self) -> String {
        "Create a new node. Use kind 'block' for note content, 'page' for named pages, 'tag' for tags. Only call when the user explicitly asks to record something.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["block", "page", "tag", "entity"] },
                "title": { "type": ["string", "null"] },
                "content": { "type": "string" }
            },
            "required": ["kind", "content"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if args.content.len() > 100_000
            || args.title.as_ref().is_some_and(|title| title.len() > 500)
        {
            return Err(ToolError::ToolCallError(
                "node title or content exceeds the allowed size".into(),
            ));
        }
        if args.kind == NodeKind::Block {
            return Err(ToolError::ToolCallError(
                "orphan blocks cannot be created; create a page instead".into(),
            ));
        }
        if args.kind == NodeKind::Page
            && args
                .title
                .as_ref()
                .is_none_or(|title| title.trim().is_empty())
        {
            return Err(ToolError::ToolCallError(
                "pages require a non-empty title".into(),
            ));
        }
        db::checkpoint_history(&self.conn, "AI create node")
            .await
            .map_err(into_tool_err)?;
        db::create_node(&self.conn, args.kind, args.title, args.content, None)
            .await
            .map_err(into_tool_err)
    }
}

// ───────────────────────── link_nodes ─────────────────────────

#[derive(Clone)]
pub struct LinkNodes {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct LinkArgs {
    pub src: i64,
    pub dst: i64,
    pub kind: String,
    #[serde(default = "default_weight")]
    pub weight: f64,
}
fn default_weight() -> f64 {
    1.0
}

#[derive(Serialize)]
pub struct LinkOk {
    ok: bool,
}

impl Tool for LinkNodes {
    const NAME: &'static str = "link_nodes";
    type Error = ToolError;
    type Args = LinkArgs;
    type Output = LinkOk;

    fn description(&self) -> String {
        "Create a typed edge between two nodes. Common kinds: 'refs', 'mentions', 'relates_to', 'contains'.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "src": { "type": "integer" },
                "dst": { "type": "integer" },
                "kind": { "type": "string" },
                "weight": { "type": "number", "default": 1.0 }
            },
            "required": ["src", "dst", "kind"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if args.src == args.dst
            || args.kind.trim().is_empty()
            || args.kind.len() > 64
            || !args.weight.is_finite()
            || !(0.0..=10.0).contains(&args.weight)
        {
            return Err(ToolError::ToolCallError(
                "invalid edge: use distinct nodes, a short kind, and weight between 0 and 10"
                    .into(),
            ));
        }
        db::checkpoint_history(&self.conn, "AI link nodes")
            .await
            .map_err(into_tool_err)?;
        db::link_nodes(&self.conn, args.src, args.dst, args.kind, args.weight)
            .await
            .map_err(into_tool_err)?;
        Ok(LinkOk { ok: true })
    }
}

// ───────────────────────── runner ─────────────────────────

fn build_agent<M: CompletionModel + 'static>(
    model: M,
    conn: Connection,
    embedder: Arc<dyn EmbedderBackend>,
    reranker: Arc<dyn RerankBackend>,
    rewriter: QueryRewriter,
    allow_writes: bool,
    active_node_id: Option<i64>,
) -> Result<rig::agent::Agent<M>, AgentError> {
    let preamble = active_node_id.map_or_else(
        || SYSTEM_PROMPT.to_string(),
        |id| {
            format!(
                "{SYSTEM_PROMPT}\nThe note currently open in the UI is node #{id}. When the user says \
                 'this note', inspect that node and its subtree instead of guessing from search."
            )
        },
    );
    let mut builder = rig::agent::AgentBuilder::new(model)
        .preamble(&preamble)
        .max_tokens(2048)
        .tool(SearchAndExpand {
            conn: conn.clone(),
            embedder: embedder.clone(),
            reranker: reranker.clone(),
            rewriter: rewriter.clone(),
        })
        .tool(SearchAgentic {
            conn: conn.clone(),
            embedder,
            reranker,
            rewriter,
        })
        .tool(ListPages { conn: conn.clone() })
        .tool(Neighbors { conn: conn.clone() })
        .tool(FindBacklinks { conn: conn.clone() })
        .tool(ReadAncestors { conn: conn.clone() })
        .tool(ReadSubtree { conn: conn.clone() })
        .tool(FindTagged { conn: conn.clone() })
        .tool(GetNode { conn: conn.clone() });
    if allow_writes {
        builder = builder
            .tool(CreateNode { conn: conn.clone() })
            .tool(LinkNodes { conn });
    }
    Ok(builder.build())
}

pub async fn run_chat_with_config(
    conn: Connection,
    embedder: Arc<dyn EmbedderBackend>,
    reranker: Arc<dyn RerankBackend>,
    message: String,
    config: llm_relay::ClientConfig,
) -> Result<String, AgentError> {
    match config
        .rig_client()
        .map_err(anyhow::Error::from)
        .map_err(AgentError::from)?
    {
        RigClient::OpenAi(client) => {
            run_chat_with_model(
                client.completion_model(&config.model),
                conn,
                embedder,
                reranker,
                message,
            )
            .await
        }
        RigClient::Anthropic(client) => {
            run_chat_with_model(
                client.completion_model(&config.model),
                conn,
                embedder,
                reranker,
                message,
            )
            .await
        }
    }
}

async fn run_chat_with_model<M: CompletionModel + 'static>(
    model: M,
    conn: Connection,
    embedder: Arc<dyn EmbedderBackend>,
    reranker: Arc<dyn RerankBackend>,
    message: String,
) -> Result<String, AgentError> {
    let agent = build_agent(
        model,
        conn,
        embedder,
        reranker,
        QueryRewriter::empty(),
        false,
        None,
    )?;
    agent
        .prompt(message)
        .max_turns(8)
        .await
        .map_err(|e| AgentError(format!("{e:#}")))
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ChatTurn {
    User { text: String },
    Assistant { text: String },
}

impl From<ChatTurn> for Message {
    fn from(t: ChatTurn) -> Self {
        match t {
            ChatTurn::User { text } => Message::user(text),
            ChatTurn::Assistant { text } => Message::assistant(text),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    TextDelta {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolStart {
        id: String,
        name: String,
        #[specta(type = specta_typescript::Unknown)]
        args: serde_json::Value,
    },
    ToolEnd {
        id: String,
        result: String,
    },
    Done {
        text: String,
    },
    Error {
        message: String,
    },
    Usage {
        #[serde(rename = "inputTokens")]
        input_tokens: u64,
        #[serde(rename = "outputTokens")]
        output_tokens: u64,
        #[serde(rename = "totalTokens")]
        total_tokens: u64,
    },
    Cancelled,
}

#[allow(clippy::too_many_arguments)]
pub async fn run_chat_stream_with_config(
    conn: Connection,
    embedder: Arc<dyn EmbedderBackend>,
    reranker: Arc<dyn RerankBackend>,
    history: Vec<ChatTurn>,
    message: String,
    allow_writes: bool,
    active_node_id: Option<i64>,
    cancelled: Arc<AtomicBool>,
    emit: impl Fn(ChatEvent) + Send + Sync + 'static,
    config: llm_relay::ClientConfig,
    query_rewriting_enabled: bool,
) -> Result<String, AgentError> {
    if message.trim().is_empty() || message.chars().count() > 16_000 {
        return Err(AgentError(
            "message must contain 1 to 16000 characters".into(),
        ));
    }
    let history_chars = history
        .iter()
        .map(|turn| match turn {
            ChatTurn::User { text } | ChatTurn::Assistant { text } => text.chars().count(),
        })
        .sum::<usize>();
    if history.len() > 24 || history_chars > 32_000 {
        return Err(AgentError(
            "chat history exceeds the 24-turn or 32000-character budget".into(),
        ));
    }
    let emit: Arc<dyn Fn(ChatEvent) + Send + Sync> = Arc::new(emit);
    match config
        .rig_client()
        .map_err(anyhow::Error::from)
        .map_err(AgentError::from)?
    {
        RigClient::OpenAi(client) => {
            run_chat_stream_with_model(
                client.completion_model(&config.model),
                conn,
                embedder,
                reranker,
                history,
                message,
                allow_writes,
                active_node_id,
                cancelled,
                emit,
                config.clone(),
                query_rewriting_enabled,
            )
            .await
        }
        RigClient::Anthropic(client) => {
            run_chat_stream_with_model(
                client.completion_model(&config.model),
                conn,
                embedder,
                reranker,
                history,
                message,
                allow_writes,
                active_node_id,
                cancelled,
                emit,
                config.clone(),
                query_rewriting_enabled,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_chat_stream_with_model<M: CompletionModel + 'static>(
    model: M,
    conn: Connection,
    embedder: Arc<dyn EmbedderBackend>,
    reranker: Arc<dyn RerankBackend>,
    history: Vec<ChatTurn>,
    message: String,
    allow_writes: bool,
    active_node_id: Option<i64>,
    cancelled: Arc<AtomicBool>,
    emit: Arc<dyn Fn(ChatEvent) + Send + Sync>,
    query_rewriter_config: llm_relay::ClientConfig,
    query_rewriting_enabled: bool,
) -> Result<String, AgentError> {
    let rewriter = QueryRewriter::new(&history, query_rewriting_enabled, query_rewriter_config);
    let agent = build_agent(
        model,
        conn,
        embedder,
        reranker,
        rewriter,
        allow_writes,
        active_node_id,
    )?;
    let history: Vec<Message> = history.into_iter().map(Into::into).collect();

    let mut stream = agent
        .stream_prompt(message)
        .history(history)
        .max_turns(8)
        .await;

    let mut full = String::new();

    loop {
        let item = tokio::select! {
            item = stream.next() => item,
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if cancelled.load(Ordering::Acquire) {
                    emit(ChatEvent::Cancelled);
                    return Ok(full);
                }
                continue;
            }
        };
        let Some(item) = item else {
            break;
        };
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => match content {
                StreamedAssistantContent::Text(t) => {
                    full.push_str(&t.text);
                    emit(ChatEvent::TextDelta { text: t.text });
                }
                StreamedAssistantContent::Reasoning(r) => {
                    let text = r
                        .content
                        .into_iter()
                        .map(|c| match c {
                            rig::message::ReasoningContent::Text { text, .. } => text,
                            rig::message::ReasoningContent::Summary(s) => s,
                            _ => String::new(),
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    emit(ChatEvent::Reasoning { text });
                }
                StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                    emit(ChatEvent::Reasoning { text: reasoning });
                }
                StreamedAssistantContent::ToolCall { tool_call, .. } => {
                    emit(ChatEvent::ToolStart {
                        id: tool_call.id.clone(),
                        name: tool_call.function.name,
                        args: tool_call.function.arguments,
                    });
                }
                StreamedAssistantContent::ToolCallDelta { .. } => {}
                StreamedAssistantContent::Final(_) => {}
                StreamedAssistantContent::Unknown(_) => {}
            },
            Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                tool_result,
                ..
            })) => {
                let result = serde_json::to_string(&tool_result.content).unwrap_or_default();
                emit(ChatEvent::ToolEnd {
                    id: tool_result.id,
                    result,
                });
            }
            Ok(MultiTurnStreamItem::FinalResponse(response)) => {
                let usage = response.usage;
                emit(ChatEvent::Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    total_tokens: usage.total_tokens,
                });
            }
            Ok(_) => {}
            Err(e) => {
                let msg = format!("{e:#}");
                emit(ChatEvent::Error {
                    message: msg.clone(),
                });
                return Err(AgentError(msg));
            }
        }
    }

    emit(ChatEvent::Done { text: full.clone() });
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contextual_query_heuristic_avoids_unneeded_calls() {
        assert!(looks_contextual("and the second one?"));
        assert!(looks_contextual("а этот?"));
        assert!(!looks_contextual("PostgreSQL transaction isolation levels"));
    }

    #[test]
    fn search_tool_arguments_are_bounded() {
        assert!(
            validate_search_args(&SearchArgs {
                query: "rust".into(),
                limit: 8,
            })
            .is_ok()
        );
        assert!(
            validate_search_args(&SearchArgs {
                query: String::new(),
                limit: 8,
            })
            .is_err()
        );
        assert!(
            validate_search_args(&SearchArgs {
                query: "rust".into(),
                limit: MAX_SEARCH_LIMIT + 1,
            })
            .is_err()
        );
    }

    #[test]
    fn rerank_fallback_keeps_real_candidates_for_low_confidence_queries() {
        let candidates = vec![
            Node {
                id: 1,
                uuid: uuid::Uuid::from_u128(1),
                kind: NodeKind::Page,
                title: Some("First note".into()),
                content: String::new(),
                content_json: None,
                parent_id: None,
                position: None,
                created_at: 0,
                updated_at: 0,
            },
            Node {
                id: 2,
                uuid: uuid::Uuid::from_u128(2),
                kind: NodeKind::Page,
                title: Some("Second note".into()),
                content: String::new(),
                content_json: None,
                parent_id: None,
                position: None,
                created_at: 0,
                updated_at: 0,
            },
        ];
        let hits = select_reranked_hits(vec![(1, 0.02), (0, 0.01)], &candidates, 8);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].node.id, 2);
        assert!(hits[0].score < RELEVANCE_FLOOR);
    }
}
