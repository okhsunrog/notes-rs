use crate::db::{self, Node, SearchHit};
use crate::embed::{EmbedderBackend, Reranker};
use anyhow::Context;
use futures::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::{Message, Prompt, ToolDefinition};
use rig::providers::openrouter;
use rig::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingPrompt};
use rig::tool::{Tool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio_rusqlite::Connection;

pub const MODEL: &str = "deepseek/deepseek-v4-flash";

const SYSTEM_PROMPT: &str = r#"
You are an assistant embedded in a personal knowledge graph (notes-rs).
The user's notes are stored as nodes (blocks, pages, entities, tags) connected by typed edges.
You answer questions by retrieving from the graph using tools — never invent facts.

Workflow:
1. Use `search_and_expand` for most questions: it does hybrid retrieval, then
   walks the graph one hop from each seed and reranks the merged set. This is
   the default because the graph almost always adds useful context.
2. Use `search_agentic` only when you want plain text-relevance with no graph
   expansion (e.g. you're looking for exact wording).
3. Use `neighbors` or `get_node` to drill into specific nodes after you've
   identified them by id.
4. Cite node IDs (e.g. "see node #42") in your final answer.
5. If nothing relevant found, say so plainly. Do not fabricate.

You may create new nodes (`create_node`) and link them (`link_nodes`) when the user explicitly asks
to record something. Never modify existing nodes without asking.
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

// ───────────────────────── search_agentic ─────────────────────────

#[derive(Clone)]
pub struct SearchAgentic {
    pub conn: Connection,
    pub embedder: Arc<dyn EmbedderBackend>,
    pub reranker: Arc<Reranker>,
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

impl Tool for SearchAgentic {
    const NAME: &'static str = "search_agentic";
    type Error = ToolError;
    type Args = SearchArgs;
    type Output = Vec<SearchHit>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Hybrid search (BM25 + semantic + BGE rerank) over notes. Returns ranked nodes with scores. Prefer this for most queries.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query in the user's language" },
                    "limit": { "type": "integer", "description": "Max results to return (default 8)", "default": 8 }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let emb = self
            .embedder
            .embed_query(args.query.clone())
            .await
            .map_err(into_tool_err)?;
        // Pool floor: at low `limit` (e.g. 3) the default `limit*4 = 12` is
        // too narrow when BM25 and vector channels disagree. Widen the rerank
        // input so we don't starve the reranker of plausible candidates.
        let pool = (args.limit * 4).max(RERANK_POOL_MIN);
        let candidates = db::search_hybrid(&self.conn, args.query.clone(), emb, pool)
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
            .rerank(args.query, docs)
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
            .map(|(idx, score)| SearchHit {
                node: candidates[idx].node.clone(),
                score: score as f64,
            })
            .collect())
    }
}

const RERANK_POOL_MIN: u32 = 32;
const RELEVANCE_FLOOR: f64 = 0.30;
/// Cap on the merged seeds+neighbors pool before reranking. The reranker is
/// the per-query cost bottleneck; ~64 docs is fine, ~512 is sluggish.
const EXPAND_POOL_MAX: usize = 64;

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
    pub reranker: Arc<Reranker>,
}

impl Tool for SearchAndExpand {
    const NAME: &'static str = "search_and_expand";
    type Error = ToolError;
    type Args = SearchArgs;
    type Output = Vec<SearchHit>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Hybrid search then walk one graph hop from each seed (refs/mentions/relations) and rerank the merged pool. Prefer this over `search_agentic` for most questions — the extra context usually helps.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query in the user's language" },
                    "limit": { "type": "integer", "description": "Max results to return (default 8)", "default": 8 }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let emb = self
            .embedder
            .embed_query(args.query.clone())
            .await
            .map_err(into_tool_err)?;
        let seed_pool = (args.limit * 4).max(RERANK_POOL_MIN);
        let seeds = db::search_hybrid(&self.conn, args.query.clone(), emb, seed_pool)
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
            .map(|n| {
                format!("{}\n{}", n.title.as_deref().unwrap_or(""), n.content)
            })
            .collect();
        let scored = self
            .reranker
            .rerank(args.query, docs)
            .await
            .map_err(into_tool_err)?;
        Ok(scored
            .into_iter()
            .filter(|(_, s)| *s as f64 >= RELEVANCE_FLOOR)
            .take(args.limit as usize)
            .map(|(idx, score)| SearchHit {
                node: candidates[idx].clone(),
                score: score as f64,
            })
            .collect())
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

impl Tool for Neighbors {
    const NAME: &'static str = "neighbors";
    type Error = ToolError;
    type Args = NeighborsArgs;
    type Output = Vec<Node>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Return nodes reachable from the given node id within `depth` hops over the graph edges (undirected).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Source node id" },
                    "depth": { "type": "integer", "description": "Hop limit (default 1)", "default": 1 }
                },
                "required": ["id"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        db::neighbors(&self.conn, args.id, args.depth)
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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Fetch a single node by its integer id.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "integer" } },
                "required": ["id"]
            }),
        }
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
    pub kind: String,
    pub title: Option<String>,
    pub content: String,
}

impl Tool for CreateNode {
    const NAME: &'static str = "create_node";
    type Error = ToolError;
    type Args = CreateNodeArgs;
    type Output = Node;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Create a new node. Use kind 'block' for note content, 'page' for named pages, 'tag' for tags. Only call when the user explicitly asks to record something.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["block", "page", "tag", "entity"] },
                    "title": { "type": ["string", "null"] },
                    "content": { "type": "string" }
                },
                "required": ["kind", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
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

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Create a typed edge between two nodes. Common kinds: 'refs', 'mentions', 'relates_to', 'contains'.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "src": { "type": "integer" },
                    "dst": { "type": "integer" },
                    "kind": { "type": "string" },
                    "weight": { "type": "number", "default": 1.0 }
                },
                "required": ["src", "dst", "kind"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        db::link_nodes(&self.conn, args.src, args.dst, args.kind, args.weight)
            .await
            .map_err(into_tool_err)?;
        Ok(LinkOk { ok: true })
    }
}

// ───────────────────────── runner ─────────────────────────

fn build_agent(
    conn: Connection,
    embedder: Arc<dyn EmbedderBackend>,
    reranker: Arc<Reranker>,
) -> Result<rig::agent::Agent<openrouter::CompletionModel>, AgentError> {
    let client = openrouter::Client::from_env()
        .context("OPENROUTER_API_KEY not set")
        .map_err(AgentError::from)?;
    Ok(client
        .agent(MODEL)
        .preamble(SYSTEM_PROMPT)
        .max_tokens(2048)
        .tool(SearchAndExpand {
            conn: conn.clone(),
            embedder: embedder.clone(),
            reranker: reranker.clone(),
        })
        .tool(SearchAgentic {
            conn: conn.clone(),
            embedder,
            reranker,
        })
        .tool(Neighbors { conn: conn.clone() })
        .tool(GetNode { conn: conn.clone() })
        .tool(CreateNode { conn: conn.clone() })
        .tool(LinkNodes { conn })
        .build())
}

pub async fn run_chat(
    conn: Connection,
    embedder: Arc<dyn EmbedderBackend>,
    reranker: Arc<Reranker>,
    message: String,
) -> Result<String, AgentError> {
    let agent = build_agent(conn, embedder, reranker)?;
    agent
        .prompt(message)
        .max_turns(8)
        .await
        .map_err(|e| AgentError(format!("{e:#}")))
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    TextDelta { text: String },
    Reasoning { text: String },
    ToolStart { id: String, name: String, args: serde_json::Value },
    ToolEnd { id: String, result: String },
    Done { text: String },
    Error { message: String },
}

pub async fn run_chat_stream(
    conn: Connection,
    embedder: Arc<dyn EmbedderBackend>,
    reranker: Arc<Reranker>,
    history: Vec<ChatTurn>,
    message: String,
    emit: impl Fn(ChatEvent) + Send + Sync + 'static,
) -> Result<String, AgentError> {
    let agent = build_agent(conn, embedder, reranker)?;
    let history: Vec<Message> = history.into_iter().map(Into::into).collect();

    let mut stream = agent
        .stream_prompt(message)
        .with_history(history)
        .multi_turn(8)
        .await;

    let mut full = String::new();

    while let Some(item) = stream.next().await {
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
            Ok(MultiTurnStreamItem::FinalResponse(_)) => {}
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
