use crate::{
    AcceptedOps, BootstrapRequest, OpsBatch, PushOps, SequencedOp, ServerInfo, SyncSnapshot,
};
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use notes_ai::agent::{ChatEvent, ChatTurn};
use notes_core::db::SearchHit;
use reqwest::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::io::ReaderStream;
use url::Url;

pub type SyncSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    base_url: Url,
    token: String,
}

impl HttpTransport {
    pub fn new(base_url: Url, token: impl Into<String>) -> Result<Self> {
        let base_url = normalize_base_url(base_url)?;
        let token = token.into();
        if token.trim().is_empty() {
            bail!("sync token cannot be empty");
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("building sync HTTP client")?;
        Ok(Self {
            client,
            base_url,
            token,
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub async fn health(&self) -> Result<()> {
        let response = self.client.get(self.endpoint("v1/health")?).send().await?;
        require_success(response).await?;
        Ok(())
    }

    pub async fn snapshot(&self) -> Result<SyncSnapshot> {
        self.get_json("v1/snapshot", &[]).await
    }

    pub async fn info(&self) -> Result<ServerInfo> {
        self.get_json("v1/info", &[]).await
    }

    pub async fn bootstrap(&self, snapshot: SyncSnapshot) -> Result<SyncSnapshot> {
        self.post_json("v1/bootstrap", &BootstrapRequest { snapshot })
            .await
    }

    pub async fn ops_since(&self, since: u64, limit: usize) -> Result<Vec<SequencedOp>> {
        let batch: OpsBatch = self
            .get_json(
                "v1/ops",
                &[("since", since.to_string()), ("limit", limit.to_string())],
            )
            .await?;
        Ok(batch.ops)
    }

    pub async fn push(&self, operations: Vec<notes_core::Op>) -> Result<Vec<SequencedOp>> {
        let accepted: AcceptedOps = self
            .post_json("v1/ops", &PushOps { ops: operations })
            .await?;
        Ok(accepted.ops)
    }

    pub async fn search(&self, query: String, limit: u32) -> Result<Vec<SearchHit>> {
        #[derive(Serialize)]
        struct Request {
            query: String,
            limit: u32,
        }
        self.post_json("v1/search", &Request { query, limit }).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn chat_stream(
        &self,
        history: Vec<ChatTurn>,
        message: String,
        allow_writes: bool,
        active_node_uuid: Option<uuid::Uuid>,
        cancelled: Arc<AtomicBool>,
        mut on_event: impl FnMut(ChatEvent),
    ) -> Result<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Request {
            history: Vec<ChatTurn>,
            message: String,
            allow_writes: bool,
            active_node_uuid: Option<uuid::Uuid>,
        }

        let response = self
            .client
            .post(self.endpoint("v1/chat")?)
            .bearer_auth(&self.token)
            .timeout(std::time::Duration::from_secs(300))
            .json(&Request {
                history,
                message,
                allow_writes,
                active_node_uuid,
            })
            .send()
            .await?;
        let response = require_success(response).await?;
        let mut body = response.bytes_stream();
        let mut buffer = String::new();
        let mut answer = None;
        let mut remote_error = None;
        loop {
            let chunk = tokio::select! {
                chunk = body.next() => chunk,
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    if cancelled.load(Ordering::Acquire) {
                        on_event(ChatEvent::Cancelled);
                        bail!("chat request was cancelled");
                    }
                    continue;
                }
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk.context("reading chat event stream")?;
            buffer.push_str(
                std::str::from_utf8(&chunk).context("chat event stream was not valid UTF-8")?,
            );
            while let Some(end) = buffer.find("\n\n") {
                let frame = buffer[..end].to_owned();
                buffer.drain(..end + 2);
                let data = frame
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect::<Vec<_>>()
                    .join("\n");
                if data.is_empty() {
                    continue;
                }
                let event: ChatEvent =
                    serde_json::from_str(&data).context("decoding chat event")?;
                match &event {
                    ChatEvent::Done { text } => answer = Some(text.clone()),
                    ChatEvent::Error { message } => remote_error = Some(message.clone()),
                    _ => {}
                }
                on_event(event);
            }
        }
        if let Some(message) = remote_error {
            bail!("{message}");
        }
        answer.context("chat stream ended without a completion event")
    }

    pub async fn connect(&self, since: u64) -> Result<SyncSocket> {
        let mut url = self.endpoint("v1/sync")?;
        url.set_scheme(match url.scheme() {
            "http" => "ws",
            "https" => "wss",
            _ => unreachable!("base URL scheme was validated"),
        })
        .map_err(|()| anyhow::anyhow!("could not construct sync websocket URL"))?;
        url.query_pairs_mut()
            .append_pair("since", &since.to_string());
        let mut request = url
            .as_str()
            .into_client_request()
            .context("building sync websocket request")?;
        request.headers_mut().insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.token)
                .parse()
                .context("sync token cannot be represented as an HTTP header")?,
        );
        let (socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .context("connecting sync websocket")?;
        Ok(socket)
    }

    pub async fn upload_blob(&self, hash: &str, path: &Path) -> Result<()> {
        validate_blob_hash(hash)?;
        let file = tokio::fs::File::open(path)
            .await
            .with_context(|| format!("opening attachment {}", path.display()))?;
        let response = self
            .client
            .put(self.endpoint(&format!("v1/blobs/{hash}"))?)
            .bearer_auth(&self.token)
            .body(reqwest::Body::wrap_stream(ReaderStream::new(file)))
            .send()
            .await?;
        require_success(response).await?;
        Ok(())
    }

    pub async fn download_blob(&self, hash: &str, path: &Path, maximum: u64) -> Result<()> {
        validate_blob_hash(hash)?;
        let response = self
            .client
            .get(self.endpoint(&format!("v1/blobs/{hash}"))?)
            .bearer_auth(&self.token)
            .send()
            .await?;
        let response = require_success(response).await?;
        if response.content_length().is_some_and(|size| size > maximum) {
            bail!("remote attachment exceeds the {maximum} byte limit");
        }
        let parent = path
            .parent()
            .context("attachment destination has no parent")?;
        tokio::fs::create_dir_all(parent).await?;
        let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::now_v7()));
        let mut file = tokio::fs::File::create(&temporary).await?;
        let mut digest = Sha256::new();
        let mut size = 0_u64;
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            size = size
                .checked_add(chunk.len() as u64)
                .context("attachment size overflow")?;
            if size > maximum {
                let _ = tokio::fs::remove_file(&temporary).await;
                bail!("remote attachment exceeds the {maximum} byte limit");
            }
            digest.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        let actual = format!("{:x}", digest.finalize());
        if actual != hash {
            let _ = tokio::fs::remove_file(&temporary).await;
            bail!("downloaded attachment hash does not match its operation");
        }
        if tokio::fs::try_exists(path).await? {
            tokio::fs::remove_file(&temporary).await?;
        } else {
            tokio::fs::rename(&temporary, path).await?;
        }
        Ok(())
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let response = self
            .client
            .get(self.endpoint(endpoint)?)
            .bearer_auth(&self.token)
            .query(query)
            .send()
            .await?;
        decode_json(response).await
    }

    async fn post_json<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &impl serde::Serialize,
    ) -> Result<T> {
        let response = self
            .client
            .post(self.endpoint(endpoint)?)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?;
        decode_json(response).await
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        self.base_url
            .join(path)
            .with_context(|| format!("joining sync endpoint {path}"))
    }
}

fn normalize_base_url(mut url: Url) -> Result<Url> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("sync server URL must use HTTP or HTTPS");
    }
    if url.host_str().is_none() {
        bail!("sync server URL must include a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("sync server URL cannot include credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("sync server URL cannot include a query or fragment");
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn validate_blob_hash(hash: &str) -> Result<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        bail!("blob hash must be 64 lowercase hexadecimal characters");
    }
    Ok(())
}

async fn decode_json<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let response = require_success(response).await?;
    response.json().await.context("decoding sync response")
}

async fn require_success(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| "response body unavailable".into());
    let message = body.chars().take(500).collect::<String>();
    match status {
        StatusCode::UNAUTHORIZED => bail!("sync authentication failed"),
        StatusCode::CONFLICT => bail!("sync conflict: {message}"),
        _ => bail!("sync server returned HTTP {status}: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_base_paths_and_rejects_embedded_credentials() {
        let transport = HttpTransport::new(
            Url::parse("https://notes.example.test/api").expect("URL"),
            "token",
        )
        .expect("transport");
        assert_eq!(
            transport.endpoint("v1/ops").expect("endpoint").as_str(),
            "https://notes.example.test/api/v1/ops"
        );
        assert!(
            HttpTransport::new(
                Url::parse("https://user:pass@notes.example.test").expect("URL"),
                "token",
            )
            .is_err()
        );
    }
}
