//! The graph as an MCP server.
//!
//! Exposes the same workspace the sync API serves, as tools an external
//! assistant can call. Writes go through [`UserState::write`], so an operation
//! is in the oplog by the time the tool answers and the note is on the user's
//! devices moments later; see the server-authored writes section of
//! `docs/architecture/sync-storage-ai.md`.
//!
//! Authentication is the server's existing bearer token: the endpoint sits
//! behind the same middleware as `/v1/ops`, and the authenticated user arrives
//! through the request extensions.

use crate::ai::AiRuntime;
use crate::api::AuthenticatedUser;
use crate::state::UserState;
use notes_core::db::{self, Block, Content, Page};
use notes_core::{BlockStyle, JournalDate, PageListFilter};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{RoleServer, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

pub type McpService = StreamableHttpService<Tangleaf, LocalSessionManager>;

/// Builds the MCP endpoint. It holds no per-user state: every call resolves its
/// account from the request, so one service serves every account exactly as the
/// JSON routes do.
pub fn service(allowed_hosts: Vec<String>, ai: Option<Arc<AiRuntime>>) -> McpService {
    let config = StreamableHttpServerConfig::default().with_allowed_hosts(allowed_hosts);
    StreamableHttpService::new(
        move || Ok(Tangleaf { ai: ai.clone() }),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

#[derive(Clone)]
pub struct Tangleaf {
    /// Present only when the deployment has an AI provider configured, which is
    /// what separates semantic search from the lexical fallback.
    ai: Option<Arc<AiRuntime>>,
}

fn invalid(message: impl Into<String>) -> ErrorData {
    ErrorData::invalid_params(message.into(), None)
}

/// Turns a domain failure into something the caller can act on.
///
/// A stale revision in particular has a remedy — read the item again and retry
/// with the revision that comes back — and saying so is the difference between
/// an assistant that recovers and one that reports a server error to the user.
fn failed(error: anyhow::Error) -> ErrorData {
    match error.downcast_ref::<notes_core::CoreError>() {
        Some(notes_core::CoreError::Conflict(message)) => ErrorData::invalid_request(
            format!(
                "{message}. Read the page or block again for its current revision, decide whether \
                 the change still applies, and retry with that revision."
            ),
            None,
        ),
        Some(notes_core::CoreError::NotFound(message)) => {
            ErrorData::resource_not_found(message.clone(), None)
        }
        Some(notes_core::CoreError::InvalidInput(message)) => {
            ErrorData::invalid_params(message.clone(), None)
        }
        _ => ErrorData::internal_error(format!("{error:#}"), None),
    }
}

/// Resolves the account this call belongs to.
///
/// The bearer token was already checked by the middleware in front of the
/// endpoint; this only reads what it left behind. A missing user means the
/// service was mounted outside that middleware, which is a wiring bug rather
/// than something a caller can provoke.
fn caller(context: &RequestContext<RoleServer>) -> Result<Arc<UserState>, ErrorData> {
    context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<AuthenticatedUser>())
        .map(|user| user.0.clone())
        .ok_or_else(|| {
            ErrorData::internal_error(
                "the MCP endpoint is missing its authenticated user".to_owned(),
                None,
            )
        })
}

fn parse_uuid(value: &str) -> Result<uuid::Uuid, ErrorData> {
    uuid::Uuid::parse_str(value).map_err(|_| invalid(format!("`{value}` is not a UUID")))
}

fn parse_revision(value: &str) -> Result<notes_core::ContentRevision, ErrorData> {
    notes_core::ContentRevision::from_str(value).map_err(|_| {
        invalid(format!(
            "`{value}` is not a revision; use the `revision` field returned when reading the item"
        ))
    })
}

fn parse_journal_date(value: &str) -> Result<JournalDate, ErrorData> {
    JournalDate::from_str(value)
        .map_err(|_| invalid(format!("`{value}` is not a date in YYYY-MM-DD form")))
}

fn checked_limit(limit: Option<u32>) -> Result<u32, ErrorData> {
    let limit = limit.unwrap_or(20);
    if (1..=200).contains(&limit) {
        Ok(limit)
    } else {
        Err(invalid("limit must be between 1 and 200"))
    }
}

/// The retrieval pipeline caps a semantic query at a hundred hits, so this
/// refuses above that rather than letting the pipeline reject it later.
fn checked_retrieval_limit(limit: Option<u32>) -> Result<u32, ErrorData> {
    let limit = limit.unwrap_or(10);
    if (1..=100).contains(&limit) {
        Ok(limit)
    } else {
        Err(invalid("limit must be between 1 and 100"))
    }
}

fn checked_depth(depth: Option<u32>, default: u32) -> Result<u32, ErrorData> {
    let depth = depth.unwrap_or(default);
    if (1..=10).contains(&depth) {
        Ok(depth)
    } else {
        Err(invalid("depth must be between 1 and 10"))
    }
}

fn checked_markdown(markdown: &str) -> Result<(), ErrorData> {
    if markdown.len() > 100_000 {
        return Err(invalid("markdown exceeds the 100000 byte limit"));
    }
    Ok(())
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CreateNoteArgs {
    /// Title of the new page. Reused if a page with this title already exists.
    pub title: String,
    /// Optional Markdown for the note's first paragraph.
    #[serde(default)]
    pub markdown: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct AppendToJournalArgs {
    /// Journal day as `YYYY-MM-DD`. Defaults to today in the server's timezone.
    #[serde(default)]
    pub date: Option<String>,
    /// Markdown for the block to append.
    pub markdown: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct AppendBlockArgs {
    /// UUID of the page to append to.
    pub page_uuid: String,
    /// Markdown for the block to append.
    pub markdown: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// What to look for. Write a self-contained query: the index never sees the
    /// conversation, only this string.
    pub query: String,
    /// How many hits to return. 1 to 200, defaults to 20.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SemanticSearchArgs {
    /// What to look for, in the caller's own words. Write a self-contained
    /// query: the index never sees the conversation, only this string.
    pub query: String,
    /// How many hits to return. 1 to 100, defaults to 10.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Rerank the candidates for relevance. Slower, better ordered, on by
    /// default.
    #[serde(default)]
    pub rerank: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ListPagesArgs {
    /// How many pages to return. 1 to 200, defaults to 20.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct UuidArgs {
    /// UUID of a page or block.
    pub uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SubtreeArgs {
    /// UUID of the page or block to read under.
    pub uuid: String,
    /// How many levels to descend. 1 to 10, defaults to 3.
    #[serde(default)]
    pub depth: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NeighborsArgs {
    /// UUID to walk out from.
    pub uuid: String,
    /// How many hops to walk. 1 to 10, defaults to 1.
    #[serde(default)]
    pub depth: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SetBlockContentArgs {
    /// UUID of the block to rewrite.
    pub uuid: String,
    /// The Markdown that should replace the block's current contents.
    pub markdown: String,
    /// The `revision` from the block as it was read. The write is refused if
    /// the block changed since.
    pub expected_revision: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct RenamePageArgs {
    /// UUID of the page to rename.
    pub uuid: String,
    /// The new title.
    pub title: String,
    /// The `revision` from the page as it was read. The write is refused if the
    /// title changed since.
    pub expected_revision: String,
}

/// The outcome of an edit.
#[derive(Serialize, schemars::JsonSchema)]
pub struct Edit {
    /// The item as it now stands, carrying its new revision.
    pub node: Node,
    /// False when the item already held this exact text, which is success, not
    /// failure: the edit had already been made.
    pub changed: bool,
}

/// One page or block, in the shape an assistant needs.
///
/// Deliberately not the internal model: order keys and revisions are noise to a
/// caller that can only append, and pinning the wire shape here keeps the tool
/// schema from drifting every time storage changes.
#[derive(Serialize, schemars::JsonSchema)]
pub struct Node {
    /// UUID to pass back to other tools.
    pub uuid: String,
    /// Either `page` or `block`.
    pub kind: String,
    /// Title, for pages.
    pub title: Option<String>,
    /// The page a block belongs to.
    pub page_uuid: Option<String>,
    /// Markdown body, for blocks.
    pub markdown: Option<String>,
    /// The revision of the editable field — a page's title, a block's Markdown.
    ///
    /// Pass it back as `expected_revision` when editing. It is how a write
    /// proves it is replacing the text it was shown, rather than silently
    /// overwriting an edit made on another device in between.
    pub revision: String,
    /// Unix seconds of the last change.
    pub updated_at: i64,
}

/// A search result: a node plus how well it matched.
#[derive(Serialize, schemars::JsonSchema)]
pub struct Hit {
    #[serde(flatten)]
    pub node: Node,
    pub score: f64,
    /// The matching excerpt, when the search engine produced one.
    pub snippet: Option<String>,
}

impl From<notes_core::db::SearchHit> for Hit {
    fn from(hit: notes_core::db::SearchHit) -> Self {
        Self {
            node: hit.content.into(),
            score: hit.score,
            snippet: hit.snippet,
        }
    }
}

impl From<Page> for Node {
    fn from(page: Page) -> Self {
        Self {
            uuid: page.uuid.to_string(),
            kind: "page".into(),
            title: page.title,
            page_uuid: None,
            markdown: None,
            revision: page.title_revision.to_string(),
            updated_at: page.updated_at,
        }
    }
}

impl From<Block> for Node {
    fn from(block: Block) -> Self {
        Self {
            uuid: block.uuid.to_string(),
            kind: "block".into(),
            title: None,
            page_uuid: Some(block.page_uuid.to_string()),
            markdown: Some(block.markdown),
            revision: block.markdown_revision.to_string(),
            updated_at: block.updated_at,
        }
    }
}

impl From<Content> for Node {
    fn from(content: Content) -> Self {
        match content {
            Content::Page(page) => page.into(),
            Content::Block(block) => block.into(),
        }
    }
}

#[tool_router(server_handler)]
impl Tangleaf {
    /// Create a note. Returns the page, including the UUID other tools take.
    ///
    /// Titles are unique: if a page with this title already exists it is
    /// returned as-is and the markdown is appended to it rather than replacing
    /// anything. Only call this when the user asks to record something.
    #[tool(description = "Create a note with a title and optional first paragraph.")]
    async fn create_note(
        &self,
        Parameters(args): Parameters<CreateNoteArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Node>, ErrorData> {
        let title = args.title.trim().to_owned();
        if title.is_empty() || title.len() > 500 {
            return Err(invalid("title must contain 1 to 500 characters"));
        }
        checked_markdown(&args.markdown)?;
        let user = caller(&context)?;
        let markdown = args.markdown;
        user.write(|conn| async move {
            let page = db::create_page(&conn, title).await?;
            if !markdown.trim().is_empty() {
                db::create_block(
                    &conn,
                    page.uuid,
                    None,
                    None,
                    BlockStyle::Paragraph,
                    markdown,
                )
                .await?;
            }
            Ok(page)
        })
        .await
        .map(|value| Json(value.into()))
        .map_err(failed)
    }

    /// Append a block to a daily journal, creating that day's page if needed.
    ///
    /// This is the right tool for a passing thought, something to remember, or
    /// anything the user does not name a page for.
    #[tool(description = "Append a Markdown block to a daily journal page.")]
    async fn append_to_journal(
        &self,
        Parameters(args): Parameters<AppendToJournalArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Node>, ErrorData> {
        if args.markdown.trim().is_empty() {
            return Err(invalid("markdown must not be empty"));
        }
        checked_markdown(&args.markdown)?;
        let date = match args.date.as_deref() {
            Some(value) => parse_journal_date(value)?,
            None => parse_journal_date(&chrono::Local::now().format("%Y-%m-%d").to_string())?,
        };
        let user = caller(&context)?;
        let markdown = args.markdown;
        user.write(|conn| async move {
            db::append_to_journal(
                &conn,
                date,
                db::BlockContent { markdown },
                BlockStyle::Paragraph,
            )
            .await
        })
        .await
        .map(|value| Json(value.into()))
        .map_err(failed)
    }

    /// Append a block to the end of an existing page.
    #[tool(description = "Append a Markdown block to the end of a page.")]
    async fn append_block(
        &self,
        Parameters(args): Parameters<AppendBlockArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Node>, ErrorData> {
        if args.markdown.trim().is_empty() {
            return Err(invalid("markdown must not be empty"));
        }
        checked_markdown(&args.markdown)?;
        let page_uuid = parse_uuid(&args.page_uuid)?;
        let user = caller(&context)?;
        let markdown = args.markdown;
        user.write(|conn| async move {
            db::create_block(
                &conn,
                page_uuid,
                None,
                None,
                BlockStyle::Paragraph,
                markdown,
            )
            .await
        })
        .await
        .map(|value| Json(value.into()))
        .map_err(failed)
    }

    /// Replace the contents of a block.
    ///
    /// Requires the revision the block carried when it was read, so an edit
    /// built on stale text is refused rather than silently overwriting whatever
    /// the user wrote on another device meanwhile.
    #[tool(description = "Replace a block's Markdown, guarded by the revision it was read at.")]
    async fn set_block_content(
        &self,
        Parameters(args): Parameters<SetBlockContentArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Edit>, ErrorData> {
        checked_markdown(&args.markdown)?;
        let uuid = parse_uuid(&args.uuid)?;
        let expected_revision = parse_revision(&args.expected_revision)?;
        let user = caller(&context)?;
        let markdown = args.markdown;
        user.write(|conn| async move {
            db::set_block_content_if_revision(
                &conn,
                uuid,
                db::BlockContent { markdown },
                expected_revision,
            )
            .await
        })
        .await
        .map(|(block, changed, _)| {
            Json(Edit {
                node: block.into(),
                changed,
            })
        })
        .map_err(failed)
    }

    /// Rename a page.
    ///
    /// Journals refuse to be renamed: their titles come from their date.
    #[tool(description = "Rename a page, guarded by the revision it was read at.")]
    async fn rename_page(
        &self,
        Parameters(args): Parameters<RenamePageArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Edit>, ErrorData> {
        let title = args.title.trim().to_owned();
        if title.is_empty() || title.len() > 500 {
            return Err(invalid("title must contain 1 to 500 characters"));
        }
        let uuid = parse_uuid(&args.uuid)?;
        let expected_revision = parse_revision(&args.expected_revision)?;
        let user = caller(&context)?;
        user.write(|conn| async move {
            db::rename_page_if_revision(&conn, uuid, Some(title), expected_revision).await
        })
        .await
        .map(|(page, changed)| {
            Json(Edit {
                node: page.into(),
                changed,
            })
        })
        .map_err(failed)
    }

    /// Search by meaning, over the server's embeddings.
    ///
    /// This is the one to reach for by default: it finds notes that say the
    /// same thing in other words, which `search_fulltext` cannot. It needs the
    /// deployment to have an AI provider configured and its index built.
    #[tool(description = "Search notes by meaning. Requires server-side AI to be configured.")]
    async fn search(
        &self,
        Parameters(args): Parameters<SemanticSearchArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Vec<Hit>>, ErrorData> {
        let query = args.query.trim().to_owned();
        if query.is_empty() || query.chars().count() > 4_096 {
            return Err(invalid("query must contain 1 to 4096 characters"));
        }
        let limit = checked_retrieval_limit(args.limit)?;
        let user = caller(&context)?;
        let ai = self.ai.as_ref().ok_or_else(|| {
            ErrorData::invalid_request(
                "this server has no AI provider configured, so semantic search is unavailable;                  use search_fulltext instead"
                    .to_owned(),
                None,
            )
        })?;
        let hits = ai
            .search(&user.id, query, limit, args.rerank.unwrap_or(true))
            .await
            .map_err(failed)?;
        Ok(Json(hits.into_iter().map(Hit::from).collect()))
    }

    /// Full-text search over the graph.
    ///
    /// Lexical, not semantic: it matches words, so prefer concrete nouns over
    /// paraphrase. Always available, including on a server with no AI provider.
    #[tool(description = "Full-text search over page titles and block contents.")]
    async fn search_fulltext(
        &self,
        Parameters(args): Parameters<SearchArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Vec<Hit>>, ErrorData> {
        let query = args.query.trim().to_owned();
        if query.is_empty() || query.chars().count() > 4_096 {
            return Err(invalid("query must contain 1 to 4096 characters"));
        }
        let limit = checked_limit(args.limit)?;
        let user = caller(&context)?;
        let hits = db::search_fts(&user.notes, query, limit)
            .await
            .map_err(failed)?;
        Ok(Json(hits.into_iter().map(Hit::from).collect()))
    }

    /// List pages in the workspace, most recently touched first.
    #[tool(description = "List pages in the workspace.")]
    async fn list_pages(
        &self,
        Parameters(args): Parameters<ListPagesArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Vec<Node>>, ErrorData> {
        let limit = checked_limit(args.limit)?;
        let user = caller(&context)?;
        db::list_pages_filtered(&user.notes, PageListFilter::All, limit)
            .await
            .map(|found| Json(found.into_iter().map(Node::from).collect()))
            .map_err(failed)
    }

    /// Read one page or block by UUID.
    #[tool(description = "Read a single page or block by UUID.")]
    async fn get_content(
        &self,
        Parameters(args): Parameters<UuidArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Option<Node>>, ErrorData> {
        let uuid = parse_uuid(&args.uuid)?;
        let user = caller(&context)?;
        db::get_content(&user.notes, uuid)
            .await
            .map(|found| Json(found.map(Node::from)))
            .map_err(failed)
    }

    /// Read everything under a page or block, down to a depth.
    #[tool(description = "Read the block subtree under a page or block.")]
    async fn read_subtree(
        &self,
        Parameters(args): Parameters<SubtreeArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Vec<Node>>, ErrorData> {
        let uuid = parse_uuid(&args.uuid)?;
        let depth = checked_depth(args.depth, 3)?;
        let user = caller(&context)?;
        db::read_subtree(&user.notes, uuid, depth)
            .await
            .map(|found| Json(found.into_iter().map(Node::from).collect()))
            .map_err(failed)
    }

    /// Read the outline breadcrumb above a block.
    #[tool(description = "Read the ancestors of a block, outermost first.")]
    async fn read_ancestors(
        &self,
        Parameters(args): Parameters<UuidArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Vec<Node>>, ErrorData> {
        let uuid = parse_uuid(&args.uuid)?;
        let user = caller(&context)?;
        db::read_ancestors(&user.notes, uuid)
            .await
            .map(|found| Json(found.into_iter().map(Node::from).collect()))
            .map_err(failed)
    }

    /// Find what points at a page or block.
    #[tool(description = "Find backlinks to a page or block.")]
    async fn find_backlinks(
        &self,
        Parameters(args): Parameters<UuidArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Vec<Node>>, ErrorData> {
        let uuid = parse_uuid(&args.uuid)?;
        let user = caller(&context)?;
        db::find_backlinks(&user.notes, uuid)
            .await
            .map(|found| Json(found.into_iter().map(Node::from).collect()))
            .map_err(failed)
    }

    /// Walk the graph outward from a page or block, following links, block
    /// references and containment in both directions.
    #[tool(description = "Walk the graph outward from a page or block.")]
    async fn neighbors(
        &self,
        Parameters(args): Parameters<NeighborsArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<Vec<Node>>, ErrorData> {
        let uuid = parse_uuid(&args.uuid)?;
        let depth = checked_depth(args.depth, 1)?;
        let user = caller(&context)?;
        db::neighbors(&user.notes, uuid, depth)
            .await
            .map(|found| Json(found.into_iter().map(Node::from).collect()))
            .map_err(failed)
    }
}
