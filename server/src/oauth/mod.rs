//! An authorization server for the MCP endpoint.
//!
//! Small on purpose. It exists so that clients which can only carry a token
//! they obtained themselves — the Claude apps, where there is nowhere to paste
//! a bearer token — can reach the same workspace that `Authorization: Bearer`
//! already reaches for the sync API and the command line.
//!
//! It issues tokens; it does not hold an identity of its own. Proving who you
//! are means presenting a server token, the credential that already authorizes
//! everything else, so there is no second password to lose.
//!
//! The shape follows OAuth 2.1 as the MCP authorization spec profiles it:
//! authorization code with mandatory S256 PKCE, public clients registered
//! dynamically, rotating refresh tokens, and discovery documents at the paths
//! clients probe.

pub mod store;

use crate::api::AppState;
use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use store::{AuthorizationCode, Client, OAuthStore, TokenClaims};

/// The single scope this server understands. Everything a tool can reach is
/// the user's own workspace, so a finer split would describe a distinction the
/// server does not make.
const SCOPE: &str = "mcp";
/// Advertised so that Claude asks for a refresh token.
const OFFLINE_SCOPE: &str = "offline_access";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
        .route("/oauth/register", post(register))
        .route("/oauth/authorize", get(authorize_form).post(authorize))
        .route("/oauth/token", post(token))
}

/// Where this deployment answers, as clients reach it.
///
/// Everything OAuth publishes is absolute, and the values have to match what
/// the user typed into the client down to the path, so they are derived from
/// one configured origin rather than guessed from request headers — a `Host`
/// an attacker controls must not be able to redirect discovery.
#[derive(Clone)]
pub struct PublicOrigin(pub url::Url);

impl PublicOrigin {
    fn issuer(&self) -> String {
        self.0.as_str().trim_end_matches('/').to_owned()
    }

    pub fn resource(&self) -> String {
        format!("{}/mcp", self.issuer())
    }

    pub fn resource_metadata(&self) -> String {
        format!("{}/.well-known/oauth-protected-resource", self.issuer())
    }
}

fn origin(state: &AppState) -> Result<&PublicOrigin, OAuthError> {
    state.public_origin.as_ref().ok_or_else(|| {
        OAuthError::server("this server has no public_url configured, so OAuth is unavailable")
    })
}

// ───────────────────────── discovery ─────────────────────────

#[derive(Serialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    scopes_supported: Vec<String>,
    bearer_methods_supported: Vec<String>,
}

async fn protected_resource_metadata(
    State(state): State<AppState>,
) -> Result<Response, OAuthError> {
    let origin = origin(&state)?;
    Ok(axum::Json(ProtectedResourceMetadata {
        resource: origin.resource(),
        authorization_servers: vec![origin.issuer()],
        scopes_supported: vec![SCOPE.into(), OFFLINE_SCOPE.into()],
        bearer_methods_supported: vec!["header".into()],
    })
    .into_response())
}

#[derive(Serialize)]
struct AuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
    response_types_supported: Vec<String>,
    grant_types_supported: Vec<String>,
    code_challenge_methods_supported: Vec<String>,
    token_endpoint_auth_methods_supported: Vec<String>,
    scopes_supported: Vec<String>,
}

async fn authorization_server_metadata(
    State(state): State<AppState>,
) -> Result<Response, OAuthError> {
    let issuer = origin(&state)?.issuer();
    Ok(axum::Json(AuthorizationServerMetadata {
        authorization_endpoint: format!("{issuer}/oauth/authorize"),
        token_endpoint: format!("{issuer}/oauth/token"),
        registration_endpoint: format!("{issuer}/oauth/register"),
        issuer,
        response_types_supported: vec!["code".into()],
        grant_types_supported: vec!["authorization_code".into(), "refresh_token".into()],
        code_challenge_methods_supported: vec!["S256".into()],
        token_endpoint_auth_methods_supported: vec!["none".into()],
        scopes_supported: vec![SCOPE.into(), OFFLINE_SCOPE.into()],
    })
    .into_response())
}

// ───────────────────────── registration ─────────────────────────

#[derive(Deserialize)]
struct RegisterRequest {
    #[serde(default)]
    client_name: Option<String>,
    redirect_uris: Vec<String>,
}

#[derive(Serialize)]
struct RegisterResponse {
    client_id: String,
    client_name: String,
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: &'static str,
    grant_types: Vec<String>,
    response_types: Vec<String>,
    client_id_issued_at: i64,
}

/// Dynamic client registration.
///
/// Open by necessity: the Claude apps register themselves at connection time
/// and there is nobody to pre-share a client id with. Registering is harmless
/// on its own — a client id authorizes nothing until a human completes the
/// consent flow with a valid server token.
async fn register(
    State(state): State<AppState>,
    axum::Json(request): axum::Json<RegisterRequest>,
) -> Result<Response, OAuthError> {
    let store = store(&state)?;
    if request.redirect_uris.is_empty() {
        return Err(OAuthError::invalid_request(
            "redirect_uris must not be empty",
        ));
    }
    for uri in &request.redirect_uris {
        let parsed = url::Url::parse(uri)
            .map_err(|_| OAuthError::invalid_request(format!("`{uri}` is not a URL")))?;
        let loopback = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if parsed.scheme() != "https" && !loopback {
            return Err(OAuthError::invalid_request(
                "redirect URIs must use https, except on loopback",
            ));
        }
    }
    let client = Client {
        client_id: format!("mcp-{}", uuid::Uuid::new_v4()),
        client_name: request
            .client_name
            .unwrap_or_else(|| "an unnamed client".into()),
        redirect_uris: request.redirect_uris,
    };
    store.register_client(client.clone()).await?;
    Ok((
        StatusCode::CREATED,
        axum::Json(RegisterResponse {
            client_id: client.client_id,
            client_name: client.client_name,
            redirect_uris: client.redirect_uris,
            token_endpoint_auth_method: "none",
            grant_types: vec!["authorization_code".into(), "refresh_token".into()],
            response_types: vec!["code".into()],
            client_id_issued_at: chrono::Utc::now().timestamp(),
        }),
    )
        .into_response())
}

// ───────────────────────── authorization ─────────────────────────

#[derive(Deserialize, Clone)]
struct AuthorizeQuery {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    code_challenge: String,
    #[serde(default)]
    code_challenge_method: Option<String>,
}

#[derive(Deserialize)]
struct AuthorizeSubmit {
    #[serde(flatten)]
    query: AuthorizeQuery,
    token: String,
}

/// Checks everything that must hold before a redirect can be trusted.
///
/// Until the client and its redirect URI are known good, an error cannot be
/// sent to that URI — doing so would turn this endpoint into an open redirect —
/// so failures here render a page instead.
async fn checked_request(state: &AppState, query: &AuthorizeQuery) -> Result<Client, OAuthError> {
    let store = store(state)?;
    if query.response_type != "code" {
        return Err(OAuthError::invalid_request(
            "only the authorization code flow is supported",
        ));
    }
    if query.code_challenge_method.as_deref().unwrap_or("plain") != "S256" {
        return Err(OAuthError::invalid_request(
            "PKCE with code_challenge_method=S256 is required",
        ));
    }
    if query.code_challenge.len() < 43 {
        return Err(OAuthError::invalid_request("code_challenge is too short"));
    }
    let client = store
        .client(&query.client_id)
        .await?
        .ok_or_else(|| OAuthError::invalid_request("unknown client_id"))?;
    if !client
        .redirect_uris
        .iter()
        .any(|registered| redirect_matches(registered, &query.redirect_uri))
    {
        return Err(OAuthError::invalid_request(
            "redirect_uri does not match one registered by this client",
        ));
    }
    Ok(client)
}

/// Compares a redirect URI against a registered one.
///
/// Exact everywhere except the loopback port: a native client binds whatever
/// port is free when it starts, so RFC 8252 has the authorization server ignore
/// it. Everything else — scheme, host, path, query — must match exactly, or the
/// registration would not be constraining anything.
fn redirect_matches(registered: &str, candidate: &str) -> bool {
    if registered == candidate {
        return true;
    }
    let (Ok(registered), Ok(candidate)) = (url::Url::parse(registered), url::Url::parse(candidate))
    else {
        return false;
    };
    let loopback = matches!(
        registered.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]")
    );
    loopback
        && registered.scheme() == candidate.scheme()
        && registered.host_str() == candidate.host_str()
        && registered.path() == candidate.path()
        && registered.query() == candidate.query()
}

async fn authorize_form(
    State(state): State<AppState>,
    Query(query): Query<AuthorizeQuery>,
) -> Result<Response, OAuthError> {
    let client = checked_request(&state, &query).await?;
    Ok(Html(consent_page(&client, &query, None)).into_response())
}

async fn authorize(
    State(state): State<AppState>,
    Form(submit): Form<AuthorizeSubmit>,
) -> Result<Response, OAuthError> {
    let client = checked_request(&state, &submit.query).await?;
    let Some(user) = state.registry.authenticate(submit.token.trim()) else {
        return Ok((
            StatusCode::UNAUTHORIZED,
            Html(consent_page(
                &client,
                &submit.query,
                Some("That is not a valid server token."),
            )),
        )
            .into_response());
    };

    let code = secret();
    store(&state)?
        .issue_code(
            &code,
            AuthorizationCode {
                client_id: client.client_id,
                user_id: user.id.clone(),
                redirect_uri: submit.query.redirect_uri.clone(),
                code_challenge: submit.query.code_challenge.clone(),
                scope: granted_scope(submit.query.scope.as_deref()),
            },
        )
        .await?;

    let mut location = url::Url::parse(&submit.query.redirect_uri)
        .map_err(|_| OAuthError::invalid_request("redirect_uri is not a URL"))?;
    location.query_pairs_mut().append_pair("code", &code);
    if let Some(state) = &submit.query.state {
        location.query_pairs_mut().append_pair("state", state);
    }
    Ok(redirect_to(location.as_str()))
}

fn redirect_to(location: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

/// Grants only what this server knows, whatever was asked for.
fn granted_scope(requested: Option<&str>) -> String {
    let offline = requested
        .map(|scope| scope.split_whitespace().any(|part| part == OFFLINE_SCOPE))
        .unwrap_or(false);
    if offline {
        format!("{SCOPE} {OFFLINE_SCOPE}")
    } else {
        SCOPE.to_owned()
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The consent screen.
///
/// It names the client and, more importantly, the host the code would be sent
/// to: that is the part an attacker would have to change, and the only part a
/// person can check. A loopback redirect gets a warning of its own, because any
/// local program can claim to be a client listening on a port.
fn consent_page(client: &Client, query: &AuthorizeQuery, error: Option<&str>) -> String {
    let redirect = url::Url::parse(&query.redirect_uri).ok();
    let host = redirect
        .as_ref()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| query.redirect_uri.clone());
    let loopback = matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]");
    let warning = if loopback {
        "<p class=\"warn\">This is a program running on your own computer. Any local program \
         can listen on a port and ask for access — continue only if you just started this \
         sign-in yourself.</p>"
    } else {
        ""
    };
    let error = error.map_or(String::new(), |message| {
        format!("<p class=\"error\">{}</p>", escape(message))
    });
    let hidden = [
        ("response_type", query.response_type.as_str()),
        ("client_id", query.client_id.as_str()),
        ("redirect_uri", query.redirect_uri.as_str()),
        ("code_challenge", query.code_challenge.as_str()),
        (
            "code_challenge_method",
            query.code_challenge_method.as_deref().unwrap_or("S256"),
        ),
        ("scope", query.scope.as_deref().unwrap_or(SCOPE)),
        ("state", query.state.as_deref().unwrap_or("")),
    ]
    .into_iter()
    .map(|(name, value)| {
        format!(
            "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
            escape(value)
        )
    })
    .collect::<String>();

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Connect to Tangleaf</title>
<style>
  :root {{ color-scheme: light dark; }}
  body {{ font: 16px/1.5 system-ui, sans-serif; margin: 0; display: grid;
          place-items: center; min-height: 100vh; padding: 1.5rem; }}
  main {{ max-width: 26rem; width: 100%; }}
  h1 {{ font-size: 1.25rem; }}
  .host {{ font-weight: 600; overflow-wrap: anywhere; }}
  .warn, .error {{ padding: .75rem; border-radius: .5rem; }}
  .warn {{ background: color-mix(in srgb, orange 20%, transparent); }}
  .error {{ background: color-mix(in srgb, red 20%, transparent); }}
  label {{ display: block; margin: 1.25rem 0 .35rem; }}
  input[type=password] {{ width: 100%; padding: .6rem; font: inherit;
                          border-radius: .5rem; border: 1px solid gray; }}
  button {{ margin-top: 1.25rem; padding: .6rem 1.2rem; font: inherit;
            border-radius: .5rem; border: 0; background: #2563eb; color: white; }}
</style>
</head>
<body>
<main>
  <h1>Give {name} access to your notes?</h1>
  <p>It will be able to read, create and edit everything in this workspace.</p>
  <p>After you approve, you will be sent to <span class="host">{host}</span>.</p>
  {warning}
  {error}
  <form method="post">
    {hidden}
    <label for="token">Server token</label>
    <input id="token" name="token" type="password" autocomplete="current-password" required
           autofocus>
    <button type="submit">Approve</button>
  </form>
</main>
</body>
</html>"#,
        name = escape(&client.client_name),
        host = escape(&host),
    )
}

// ───────────────────────── tokens ─────────────────────────

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    code_verifier: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: i64,
    refresh_token: Option<String>,
    scope: String,
}

async fn token(
    State(state): State<AppState>,
    Form(request): Form<TokenRequest>,
) -> Result<Response, OAuthError> {
    match request.grant_type.as_str() {
        "authorization_code" => exchange_code(&state, request).await,
        "refresh_token" => refresh(&state, request).await,
        other => Err(OAuthError::unsupported_grant_type(format!(
            "`{other}` is not a supported grant type"
        ))),
    }
}

async fn exchange_code(state: &AppState, request: TokenRequest) -> Result<Response, OAuthError> {
    let store = store(state)?;
    let code = request
        .code
        .ok_or_else(|| OAuthError::invalid_request("code is required"))?;
    let verifier = request
        .code_verifier
        .ok_or_else(|| OAuthError::invalid_request("code_verifier is required"))?;
    let claims = store
        .redeem_code(&code)
        .await?
        .ok_or_else(|| OAuthError::invalid_grant("the authorization code is unknown or expired"))?;

    if request
        .client_id
        .as_deref()
        .is_some_and(|client_id| client_id != claims.client_id)
    {
        return Err(OAuthError::invalid_grant(
            "the code was issued to a different client",
        ));
    }
    if request
        .redirect_uri
        .as_deref()
        .is_some_and(|uri| uri != claims.redirect_uri)
    {
        return Err(OAuthError::invalid_grant(
            "redirect_uri does not match the one the code was issued for",
        ));
    }
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    if challenge != claims.code_challenge {
        return Err(OAuthError::invalid_grant(
            "the code_verifier does not match",
        ));
    }

    issue(
        state,
        TokenClaims {
            client_id: claims.client_id,
            user_id: claims.user_id,
            scope: claims.scope,
        },
    )
    .await
}

async fn refresh(state: &AppState, request: TokenRequest) -> Result<Response, OAuthError> {
    let store = store(state)?;
    let presented = request
        .refresh_token
        .ok_or_else(|| OAuthError::invalid_request("refresh_token is required"))?;
    let claims = store
        .token(&presented, "refresh")
        .await?
        .ok_or_else(|| OAuthError::invalid_grant("the refresh token is unknown or expired"))?;
    if request
        .client_id
        .as_deref()
        .is_some_and(|client_id| client_id != claims.client_id)
    {
        return Err(OAuthError::invalid_grant(
            "the refresh token belongs to a different client",
        ));
    }
    // Rotation: the presented token stops working in the same exchange that
    // hands out its replacement.
    store.revoke_token(&presented).await?;
    issue(state, claims).await
}

async fn issue(state: &AppState, claims: TokenClaims) -> Result<Response, OAuthError> {
    let store = store(state)?;
    let access = secret();
    store.issue_token(&access, "access", claims.clone()).await?;
    let refresh = if claims.scope.split_whitespace().any(|s| s == OFFLINE_SCOPE) {
        let refresh = secret();
        store
            .issue_token(&refresh, "refresh", claims.clone())
            .await?;
        Some(refresh)
    } else {
        None
    };
    let mut response = axum::Json(TokenResponse {
        access_token: access,
        token_type: "Bearer",
        expires_in: store::ACCESS_TOKEN_TTL,
        refresh_token: refresh,
        scope: claims.scope,
    })
    .into_response();
    // RFC 6749 §5.1: token responses must not be cached.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn store(state: &AppState) -> Result<&OAuthStore, OAuthError> {
    state.oauth.as_ref().ok_or_else(|| {
        OAuthError::server("this server has no public_url configured, so OAuth is unavailable")
    })
}

/// 256 bits from the system generator, in the URL-safe alphabet so it survives
/// every place a token gets carried.
fn secret() -> String {
    let mut bytes = [0_u8; 32];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

// ───────────────────────── errors ─────────────────────────

/// An OAuth error, in the shape RFC 6749 defines.
///
/// The codes matter beyond tidiness: a client refreshing a token decides
/// whether to re-authorize or to give up based on seeing `invalid_grant`
/// specifically, so a generic failure here turns a recoverable expiry into a
/// broken connection.
pub struct OAuthError {
    status: StatusCode,
    code: &'static str,
    description: String,
}

impl OAuthError {
    fn invalid_request(description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            description: description.into(),
        }
    }

    fn invalid_grant(description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_grant",
            description: description.into(),
        }
    }

    fn unsupported_grant_type(description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "unsupported_grant_type",
            description: description.into(),
        }
    }

    fn server(description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "server_error",
            description: description.into(),
        }
    }
}

impl From<anyhow::Error> for OAuthError {
    fn from(error: anyhow::Error) -> Self {
        tracing::error!(?error, "the authorization server failed");
        Self::server("the authorization server failed")
    }
}

#[derive(Serialize)]
struct OAuthErrorBody {
    error: &'static str,
    error_description: String,
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        (
            self.status,
            axum::Json(OAuthErrorBody {
                error: self.code,
                error_description: self.description,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::redirect_matches;

    #[test]
    fn loopback_redirects_ignore_the_port_and_nothing_else() {
        // A native client binds whatever port is free when it starts, so it
        // registers a port-less loopback URI and arrives on an ephemeral one.
        assert!(redirect_matches(
            "http://localhost/callback",
            "http://localhost:3118/callback"
        ));
        assert!(redirect_matches(
            "http://127.0.0.1/callback",
            "http://127.0.0.1:51000/callback"
        ));
        assert!(redirect_matches(
            "http://localhost:3118/callback",
            "http://localhost:3118/callback"
        ));

        // The licence extends to the port and stops there.
        assert!(!redirect_matches(
            "http://localhost/callback",
            "http://localhost:3118/elsewhere"
        ));
        assert!(!redirect_matches(
            "http://localhost/callback",
            "http://127.0.0.1:3118/callback"
        ));
        assert!(!redirect_matches(
            "http://localhost/callback",
            "https://localhost/callback"
        ));
        assert!(!redirect_matches(
            "http://localhost/callback",
            "http://localhost:3118/callback?extra=1"
        ));

        // And it is only for loopback: a hosted client is matched exactly.
        assert!(!redirect_matches(
            "https://claude.ai/api/mcp/auth_callback",
            "https://claude.ai:8443/api/mcp/auth_callback"
        ));
        assert!(!redirect_matches(
            "https://claude.ai/api/mcp/auth_callback",
            "https://claude.ai.evil.example/api/mcp/auth_callback"
        ));
    }
}
