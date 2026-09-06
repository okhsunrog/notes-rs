//! The authorization server, walked end to end the way a client walks it.
//!
//! This is the path the Claude apps take, where there is nowhere to paste a
//! bearer token: discover, register, consent, exchange, call. The refusals
//! matter as much as the happy path — a code that works twice, a verifier that
//! is not checked, or a refresh token that outlives its rotation would each
//! hand someone else the workspace.

mod common;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::redirect::Policy;
use sha2::{Digest, Sha256};

const VERIFIER: &str = "a-code-verifier-of-entirely-sufficient-length-000000";
const REDIRECT: &str = "https://claude.ai/api/mcp/auth_callback";

fn challenge() -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()))
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .expect("http client")
}

async fn json(response: reqwest::Response) -> serde_json::Value {
    response.json().await.expect("a JSON body")
}

/// Registers a client and drives consent, returning the authorization code.
async fn authorize(harness: &common::Harness, http: &reqwest::Client) -> (String, String) {
    let registered = json(
        http.post(harness.url("/oauth/register"))
            .json(&serde_json::json!({
                "client_name": "Claude",
                "redirect_uris": [REDIRECT],
            }))
            .send()
            .await
            .expect("registering"),
    )
    .await;
    let client_id = registered["client_id"]
        .as_str()
        .expect("client_id")
        .to_owned();

    let form = [
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", REDIRECT),
        ("code_challenge", &challenge()),
        ("code_challenge_method", "S256"),
        ("scope", "mcp offline_access"),
        ("state", "opaque-client-state"),
        ("token", common::TOKEN),
    ];
    let response = http
        .post(harness.url("/oauth/authorize"))
        .form(&form)
        .send()
        .await
        .expect("consent");
    assert_eq!(response.status(), 302, "consent should redirect");
    let location = response
        .headers()
        .get("location")
        .expect("a redirect")
        .to_str()
        .expect("a text location")
        .to_owned();
    let location = url::Url::parse(&location).expect("a redirect URL");
    assert!(
        location.as_str().starts_with(REDIRECT),
        "the code went somewhere unexpected: {location}"
    );
    let pairs: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
    assert_eq!(
        pairs.get("state").map(String::as_str),
        Some("opaque-client-state"),
        "the client's state must come back untouched"
    );
    (client_id, pairs["code"].clone())
}

#[tokio::test]
async fn discovery_points_a_client_at_the_authorization_server() {
    let harness = common::start_public_server().await;
    let http = client();
    let base = harness.url("");

    // An unauthenticated call has to say where to authorize, on a 401, or a
    // client has nothing to follow.
    let refused = http
        .post(harness.url("/mcp"))
        .send()
        .await
        .expect("unauthenticated call");
    assert_eq!(refused.status(), 401);
    let challenge = refused
        .headers()
        .get("www-authenticate")
        .expect("a WWW-Authenticate header")
        .to_str()
        .expect("a text header")
        .to_owned();
    assert!(
        challenge.contains(&format!(
            "resource_metadata=\"{base}/.well-known/oauth-protected-resource\""
        )),
        "the challenge must point at the metadata: {challenge}"
    );

    let resource = json(
        http.get(harness.url("/.well-known/oauth-protected-resource"))
            .send()
            .await
            .expect("resource metadata"),
    )
    .await;
    assert_eq!(
        resource["resource"],
        serde_json::json!(format!("{base}/mcp"))
    );
    assert_eq!(
        resource["authorization_servers"],
        serde_json::json!([base.clone()])
    );

    let server = json(
        http.get(harness.url("/.well-known/oauth-authorization-server"))
            .send()
            .await
            .expect("authorization server metadata"),
    )
    .await;
    assert_eq!(server["issuer"], serde_json::json!(base));
    assert_eq!(
        server["code_challenge_methods_supported"],
        serde_json::json!(["S256"]),
        "S256 must be advertised or a spec-compliant client will not start"
    );
    assert_eq!(
        server["token_endpoint_auth_methods_supported"],
        serde_json::json!(["none"])
    );
    assert!(
        server["registration_endpoint"].is_string(),
        "dynamic registration is how the Claude apps get a client id"
    );
}

#[tokio::test]
async fn a_granted_token_reaches_the_workspace() {
    let harness = common::start_public_server().await;
    let http = client();
    let (client_id, code) = authorize(&harness, &http).await;

    let granted = json(
        http.post(harness.url("/oauth/token"))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", REDIRECT),
                ("client_id", client_id.as_str()),
                ("code_verifier", VERIFIER),
            ])
            .send()
            .await
            .expect("token exchange"),
    )
    .await;
    let access = granted["access_token"].as_str().expect("access token");
    assert_eq!(granted["token_type"], serde_json::json!("Bearer"));
    assert!(
        granted["refresh_token"].is_string(),
        "offline_access was asked for"
    );

    // The whole point: this token opens the same endpoint a server token does.
    let session = common::connect_mcp(&harness, access)
        .await
        .expect("MCP handshake with an OAuth token");
    let tools = session.list_all_tools().await.expect("listing tools");
    assert!(tools.iter().any(|tool| tool.name == "create_note"));
    session.cancel().await.expect("closing the session");

    // Rotation: refreshing hands out a new pair and retires the old refresh
    // token in the same exchange.
    let refresh = granted["refresh_token"].as_str().expect("refresh token");
    let refreshed = json(
        http.post(harness.url("/oauth/token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
                ("client_id", client_id.as_str()),
            ])
            .send()
            .await
            .expect("refresh"),
    )
    .await;
    assert!(refreshed["access_token"].is_string());
    assert_ne!(refreshed["refresh_token"], granted["refresh_token"]);

    let reused = http
        .post(harness.url("/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", client_id.as_str()),
        ])
        .send()
        .await
        .expect("reusing the retired refresh token");
    assert_eq!(reused.status(), 400);
    assert_eq!(
        json(reused).await["error"],
        serde_json::json!("invalid_grant"),
        "clients decide whether to re-authorize by this exact code"
    );
}

#[tokio::test]
async fn a_code_is_worthless_without_its_verifier() {
    let harness = common::start_public_server().await;
    let http = client();
    let (client_id, code) = authorize(&harness, &http).await;

    let stolen = http
        .post(harness.url("/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT),
            ("client_id", client_id.as_str()),
            (
                "code_verifier",
                "a-different-verifier-of-sufficient-length-00000000",
            ),
        ])
        .send()
        .await
        .expect("exchange with the wrong verifier");
    assert_eq!(stolen.status(), 400);
    assert_eq!(
        json(stolen).await["error"],
        serde_json::json!("invalid_grant")
    );
}

#[tokio::test]
async fn a_code_can_only_be_spent_once() {
    let harness = common::start_public_server().await;
    let http = client();
    let (client_id, code) = authorize(&harness, &http).await;
    let exchange = || {
        http.post(harness.url("/oauth/token")).form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT),
            ("client_id", client_id.as_str()),
            ("code_verifier", VERIFIER),
        ])
    };

    let first = exchange().send().await.expect("first exchange");
    assert_eq!(first.status(), 200);
    let second = exchange().send().await.expect("second exchange");
    assert_eq!(second.status(), 400);
    assert_eq!(
        json(second).await["error"],
        serde_json::json!("invalid_grant")
    );
}

#[tokio::test]
async fn consent_refuses_a_wrong_server_token_and_an_unregistered_redirect() {
    let harness = common::start_public_server().await;
    let http = client();
    let registered = json(
        http.post(harness.url("/oauth/register"))
            .json(&serde_json::json!({
                "client_name": "Claude",
                "redirect_uris": [REDIRECT],
            }))
            .send()
            .await
            .expect("registering"),
    )
    .await;
    let client_id = registered["client_id"].as_str().expect("client_id");

    let wrong_token = http
        .post(harness.url("/oauth/authorize"))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", REDIRECT),
            ("code_challenge", &challenge()),
            ("code_challenge_method", "S256"),
            ("token", "not-the-configured-server-token"),
        ])
        .send()
        .await
        .expect("consent with a wrong token");
    assert_eq!(wrong_token.status(), 401, "no code for an unproven caller");

    // An attacker-supplied redirect must not receive a code, and must not even
    // be redirected to: that would make this endpoint an open redirect.
    let elsewhere = http
        .post(harness.url("/oauth/authorize"))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", "https://example.invalid/steal"),
            ("code_challenge", &challenge()),
            ("code_challenge_method", "S256"),
            ("token", common::TOKEN),
        ])
        .send()
        .await
        .expect("consent with an unregistered redirect");
    assert_eq!(elsewhere.status(), 400);
    assert!(
        elsewhere.headers().get("location").is_none(),
        "an unregistered redirect must not be followed"
    );
}

#[tokio::test]
async fn plain_pkce_is_refused() {
    let harness = common::start_public_server().await;
    let http = client();
    let registered = json(
        http.post(harness.url("/oauth/register"))
            .json(&serde_json::json!({ "redirect_uris": [REDIRECT] }))
            .send()
            .await
            .expect("registering"),
    )
    .await;
    let client_id = registered["client_id"].as_str().expect("client_id");

    let downgraded = http
        .get(harness.url("/oauth/authorize"))
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", REDIRECT),
            ("code_challenge", VERIFIER),
            ("code_challenge_method", "plain"),
        ])
        .send()
        .await
        .expect("a downgraded authorization request");
    assert_eq!(downgraded.status(), 400);
}

/// A workspace can answer under more than one name. A client checks that the
/// resource it was told about matches the URL its user typed, so each name has
/// to describe itself — otherwise the second domain is reachable by token but
/// not connectable by OAuth.
#[tokio::test]
async fn each_configured_name_describes_itself() {
    let harness =
        common::start_public_server_for(vec!["127.0.0.1".into(), "notes.example.dev".into()]).await;
    let http = client();
    let canonical = harness.url("");

    let described = |host: &'static str| {
        let request = http
            .get(harness.url("/.well-known/oauth-protected-resource"))
            .header("host", host);
        async move {
            json(request.send().await.expect("resource metadata")).await["resource"]
                .as_str()
                .expect("a resource")
                .to_owned()
        }
    };

    assert_eq!(
        described("notes.example.dev").await,
        "http://notes.example.dev/mcp",
        "a configured name must describe itself"
    );

    // A name nobody configured decides nothing.
    assert_eq!(
        described("attacker.example").await,
        format!("{canonical}/mcp"),
        "an unknown Host must fall back to the canonical origin"
    );

    // And the pointer on a refusal follows the same rule, so the client is sent
    // to the document that describes the URL it used.
    let refused = http
        .post(harness.url("/mcp"))
        .header("host", "notes.example.dev")
        .send()
        .await
        .expect("unauthenticated call");
    assert_eq!(refused.status(), 401);
    assert!(
        refused
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|challenge| challenge.contains(
                "resource_metadata=\"http://notes.example.dev/.well-known/oauth-protected-resource\""
            )),
        "the challenge must point at the metadata for the name that was used"
    );
}
