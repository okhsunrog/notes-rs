use crate::retrieval::{RetrievalPipeline, content_documents, select_reranked_hits};
use anyhow::Context;
use futures::StreamExt;
use llm_relay::RigClient;
use notes_core::db::{self, Content, Page, SearchHit};
use notes_core::{BlockStyle, Connection, PageListFilter};
use notes_protocol::{ChatEvent, ChatTurn};
use rig::agent::MultiTurnStreamItem;
use rig::client::CompletionClient;
use rig::completion::{CompletionModel, Message, Prompt};
use rig::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingPrompt};
use rig::tool::{Tool, ToolError};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const SYSTEM_PROMPT: &str = r#"
You are an assistant embedded in a personal knowledge graph (notes-rs).
The user's notes are stored as UUID-addressed pages containing ordered block trees.
You answer questions by retrieving from the graph using tools — never invent facts.

Workflow:
1. Use `list_pages` for inventory questions such as "what notes do I have?",
   "list all notes", or requests for a notebook-wide overview. Then use
   `read_subtree` on the returned page ids when their contents are needed.
2. Use `search_and_expand` for relevance questions: it does hybrid retrieval, then
   walks page links, block references, and containment one hop from each seed and reranks the merged set. This is
   the default because the graph almost always adds useful context.
3. Use `search_agentic` only when you want plain text-relevance with no graph
   expansion (e.g. you're looking for exact wording).
4. Drill into specific content once you have UUIDs:
   - `read_ancestors` for the outline breadcrumb above a block,
   - `read_subtree` to read everything under a page or section,
   - `find_backlinks` for "who points at this?",
   - `neighbors` for undirected graph walks,
   - `get_content` for one page or block.
5. Cite page/block UUIDs in your final answer.
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
    pub retrieval: RetrievalPipeline,
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
        "Hybrid search (BM25 + semantic + BGE rerank) over notes. Returns ranked pages and blocks with scores. Prefer this for most queries.".into()
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
        self.retrieval
            .retrieve(query, args.limit)
            .await
            .map_err(into_tool_err)
    }
}

/// Cap on the merged seeds+neighbors pool before reranking. The reranker is
/// the per-query cost bottleneck; ~64 docs is fine, ~512 is sluggish.
const EXPAND_POOL_MAX: usize = 64;
// ───────────────────────── search_and_expand ─────────────────────────

/// Hybrid search + 1-hop graph expansion + rerank. The default retrieval
/// tool: starts from the same hybrid candidates as `search_agentic`, then
/// pulls each seed's immediate neighbors (links, references, containment) into
/// the candidate set before reranking. This is what makes the typed-edge
/// graph actually do work for the model instead of being decoration.
#[derive(Clone)]
pub struct SearchAndExpand {
    pub retrieval: RetrievalPipeline,
    pub rewriter: QueryRewriter,
}

impl Tool for SearchAndExpand {
    const NAME: &'static str = "search_and_expand";
    type Error = ToolError;
    type Args = SearchArgs;
    type Output = Vec<SearchHit>;

    fn description(&self) -> String {
        "Hybrid search then walk one graph hop from each seed (links/references/containment) and rerank the merged pool. Prefer this over `search_agentic` for most questions — the extra context usually helps.".into()
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
        let seed_pool = args
            .limit
            .saturating_mul(4)
            .max(crate::retrieval::RERANK_POOL_MIN);
        let seeds = self
            .retrieval
            .hybrid_candidates(query.clone(), seed_pool)
            .await
            .map_err(into_tool_err)?;
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        // Walk one hop from each seed; dedup by UUID, cap the merged pool.
        use std::collections::HashMap;
        let mut pool: HashMap<uuid::Uuid, Content> = HashMap::new();
        for s in &seeds {
            pool.entry(s.content.uuid())
                .or_insert_with(|| s.content.clone());
        }
        for s in &seeds {
            if pool.len() >= EXPAND_POOL_MAX {
                break;
            }
            let neigh = db::neighbors(self.retrieval.notes(), s.content.uuid(), 1)
                .await
                .map_err(into_tool_err)?;
            for n in neigh {
                if pool.len() >= EXPAND_POOL_MAX {
                    break;
                }
                pool.entry(n.uuid()).or_insert(n);
            }
        }
        let candidates = pool
            .into_values()
            .map(|content| SearchHit {
                content,
                score: 0.0,
                snippet: None,
            })
            .collect::<Vec<_>>();
        let content = candidates
            .iter()
            .map(|candidate| candidate.content.clone())
            .collect::<Vec<_>>();
        let docs = content_documents(&content);
        let scored = self
            .retrieval
            .reranker()
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
    type Output = Vec<Page>;

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
        db::list_pages_filtered(&self.conn, PageListFilter::All, args.limit)
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
    pub uuid: uuid::Uuid,
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
    type Output = Vec<Content>;

    fn description(&self) -> String {
        "Return pages and blocks reachable from a UUID within `depth` link/containment hops.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "uuid": { "type": "string", "format": "uuid", "description": "Source page or block UUID" },
                "depth": { "type": "integer", "minimum": 1, "maximum": MAX_GRAPH_DEPTH, "description": "Hop limit (default 1)", "default": 1 }
            },
            "required": ["uuid"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if args.depth == 0 || args.depth > MAX_GRAPH_DEPTH {
            return Err(ToolError::ToolCallError(
                format!("depth must be between 1 and {MAX_GRAPH_DEPTH}").into(),
            ));
        }
        db::neighbors(&self.conn, args.uuid, args.depth)
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
    pub uuid: uuid::Uuid,
}

impl Tool for FindBacklinks {
    const NAME: &'static str = "find_backlinks";
    type Error = ToolError;
    type Args = FindBacklinksArgs;
    type Output = Vec<Content>;

    fn description(&self) -> String {
        "Find blocks that link to the given page or block UUID through a wikilink or block reference.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "uuid": { "type": "string", "format": "uuid", "description": "Target page or block UUID" }
            },
            "required": ["uuid"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        db::find_backlinks(&self.conn, args.uuid)
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
    pub uuid: uuid::Uuid,
}

impl Tool for ReadAncestors {
    const NAME: &'static str = "read_ancestors";
    type Error = ToolError;
    type Args = ReadAncestorsArgs;
    type Output = Vec<Content>;

    fn description(&self) -> String {
        "Walk the parent chain from this block up to the page root. Returned root-first; the block itself is the last element. Useful for getting an outline breadcrumb / surrounding context.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "uuid": { "type": "string", "format": "uuid" } },
            "required": ["uuid"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        db::read_ancestors(&self.conn, args.uuid)
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
    pub uuid: uuid::Uuid,
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
    type Output = Vec<Content>;

    fn description(&self) -> String {
        "Return descendants of a page or block UUID in outline order.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "uuid": { "type": "string", "format": "uuid" },
                "depth": { "type": "integer", "minimum": 1, "maximum": MAX_SUBTREE_DEPTH, "description": "Max levels to descend (default 4)", "default": 4 }
            },
            "required": ["uuid"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if args.depth == 0 || args.depth > MAX_SUBTREE_DEPTH {
            return Err(ToolError::ToolCallError(
                format!("depth must be between 1 and {MAX_SUBTREE_DEPTH}").into(),
            ));
        }
        db::read_subtree(&self.conn, args.uuid, args.depth)
            .await
            .map_err(into_tool_err)
    }
}

// ───────────────────────── get_content ─────────────────────────

#[derive(Clone)]
pub struct GetContent {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct GetContentArgs {
    pub uuid: uuid::Uuid,
}

impl Tool for GetContent {
    const NAME: &'static str = "get_content";
    type Error = ToolError;
    type Args = GetContentArgs;
    type Output = Option<Content>;

    fn description(&self) -> String {
        "Fetch one page or block by UUID.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "uuid": { "type": "string", "format": "uuid" } },
            "required": ["uuid"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        db::get_content(&self.conn, args.uuid)
            .await
            .map_err(into_tool_err)
    }
}

// ───────────────────────── create_page ─────────────────────────

#[derive(Clone)]
pub struct CreatePage {
    pub conn: Connection,
}

#[derive(Deserialize)]
pub struct CreatePageArgs {
    pub title: String,
    #[serde(default)]
    pub markdown: String,
}

impl Tool for CreatePage {
    const NAME: &'static str = "create_page";
    type Error = ToolError;
    type Args = CreatePageArgs;
    type Output = Page;

    fn description(&self) -> String {
        "Create a page and optionally its first paragraph. Only call when the user explicitly asks to record something.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "minLength": 1, "maxLength": 500 },
                "markdown": { "type": "string", "maxLength": 100000, "default": "" }
            },
            "required": ["title"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if args.markdown.len() > 100_000 || args.title.trim().is_empty() || args.title.len() > 500 {
            return Err(ToolError::ToolCallError(
                "page title or content exceeds the allowed size".into(),
            ));
        }
        let page = db::create_page(&self.conn, args.title)
            .await
            .map_err(into_tool_err)?;
        if !args.markdown.trim().is_empty() {
            db::create_block(
                &self.conn,
                page.uuid,
                None,
                None,
                BlockStyle::Paragraph,
                args.markdown,
            )
            .await
            .map_err(into_tool_err)?;
        }
        Ok(page)
    }
}

// ───────────────────────── runner ─────────────────────────

fn build_agent<M: CompletionModel + 'static>(
    model: M,
    retrieval: RetrievalPipeline,
    rewriter: QueryRewriter,
    allow_writes: bool,
    active_content_uuid: Option<uuid::Uuid>,
) -> Result<rig::agent::Agent<M>, AgentError> {
    let conn = retrieval.notes().clone();
    let preamble = active_content_uuid.map_or_else(
        || SYSTEM_PROMPT.to_string(),
        |uuid| {
            format!(
                "{SYSTEM_PROMPT}\nThe page currently open in the UI has UUID {uuid}. When the user says \
                 'this note', inspect that page and its subtree instead of guessing from search."
            )
        },
    );
    let mut builder = rig::agent::AgentBuilder::new(model)
        .preamble(&preamble)
        .max_tokens(2048)
        .tool(SearchAndExpand {
            retrieval: retrieval.clone(),
            rewriter: rewriter.clone(),
        })
        .tool(SearchAgentic {
            retrieval,
            rewriter,
        })
        .tool(ListPages { conn: conn.clone() })
        .tool(Neighbors { conn: conn.clone() })
        .tool(FindBacklinks { conn: conn.clone() })
        .tool(ReadAncestors { conn: conn.clone() })
        .tool(ReadSubtree { conn: conn.clone() })
        .tool(GetContent { conn: conn.clone() });
    if allow_writes {
        builder = builder.tool(CreatePage { conn });
    }
    Ok(builder.build())
}

fn chat_turn_message(turn: ChatTurn) -> Message {
    match turn {
        ChatTurn::User { text } => Message::user(text),
        ChatTurn::Assistant { text } => Message::assistant(text),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_chat_stream_with_config(
    retrieval: RetrievalPipeline,
    history: Vec<ChatTurn>,
    message: String,
    allow_writes: bool,
    active_content_uuid: Option<uuid::Uuid>,
    cancelled: CancellationToken,
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
                retrieval,
                history,
                message,
                allow_writes,
                active_content_uuid,
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
                retrieval,
                history,
                message,
                allow_writes,
                active_content_uuid,
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
    retrieval: RetrievalPipeline,
    history: Vec<ChatTurn>,
    message: String,
    allow_writes: bool,
    active_content_uuid: Option<uuid::Uuid>,
    cancelled: CancellationToken,
    emit: Arc<dyn Fn(ChatEvent) + Send + Sync>,
    query_rewriter_config: llm_relay::ClientConfig,
    query_rewriting_enabled: bool,
) -> Result<String, AgentError> {
    let rewriter = QueryRewriter::new(&history, query_rewriting_enabled, query_rewriter_config);
    let agent = build_agent(
        model,
        retrieval,
        rewriter,
        allow_writes,
        active_content_uuid,
    )?;
    let history: Vec<Message> = history.into_iter().map(chat_turn_message).collect();

    let mut stream = agent
        .stream_prompt(message)
        .history(history)
        .max_turns(8)
        .await;

    let mut full = String::new();

    loop {
        let item = tokio::select! {
            item = stream.next() => item,
            () = cancelled.cancelled() => {
                emit(ChatEvent::Cancelled);
                return Ok(full);
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
            Content::Page(Page {
                uuid: uuid::Uuid::from_u128(1),
                kind: notes_core::PageKind::Note,
                title: Some("First note".into()),
                layout: notes_core::PageLayout::Outline,
                title_revision: "0000000000000000-00000000-00000000000000000000000000000001"
                    .parse()
                    .expect("revision"),
                created_at: 0,
                updated_at: 0,
            }),
            Content::Page(Page {
                uuid: uuid::Uuid::from_u128(2),
                kind: notes_core::PageKind::Note,
                title: Some("Second note".into()),
                layout: notes_core::PageLayout::Outline,
                title_revision: "0000000000000000-00000000-00000000000000000000000000000001"
                    .parse()
                    .expect("revision"),
                created_at: 0,
                updated_at: 0,
            }),
        ];
        let candidates = candidates
            .into_iter()
            .enumerate()
            .map(|(index, content)| SearchHit {
                content,
                score: 0.0,
                snippet: (index == 1).then(|| "matching snippet".into()),
            })
            .collect::<Vec<_>>();
        let hits = select_reranked_hits(vec![(1, 0.02), (0, 0.01)], &candidates, 8);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].content.uuid(), uuid::Uuid::from_u128(2));
        assert!(hits[0].score < crate::retrieval::RELEVANCE_FLOOR);
        assert_eq!(hits[0].snippet.as_deref(), Some("matching snippet"));
    }

    #[tokio::test]
    async fn page_inventory_includes_journal_pages() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let connection = db::open(directory.path().join("notes.db"))
            .await
            .expect("open database");
        db::create_page(&connection, "Project".into())
            .await
            .expect("create note");
        db::ensure_journal(
            &connection,
            "2026-07-17".parse().expect("valid journal date"),
        )
        .await
        .expect("create journal");

        let pages = ListPages { conn: connection }
            .call(ListPagesArgs { limit: 10 })
            .await
            .expect("list pages through agent tool");

        assert_eq!(pages.len(), 2);
        assert!(pages.iter().any(|page| page.kind.is_journal()));
    }
}
