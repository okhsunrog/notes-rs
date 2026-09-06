//! Persistent state for the authorization server: registered clients, codes in
//! flight, and issued tokens.
//!
//! Secrets are stored as SHA-256 digests and compared in constant time, so a
//! copy of this database does not hand anyone a working token. Everything here
//! is short-lived by design — codes last a minute, access tokens an hour — and
//! expired rows are swept on each lookup rather than by a timer.

use anyhow::{Context, Result};
use notes_core::Connection;
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::path::Path;
use subtle::ConstantTimeEq;

/// How long an authorization code stays usable. The client redeems it
/// immediately; a minute is generous and keeps the window small.
pub const CODE_TTL: i64 = 60;
/// How long an access token lasts before the client refreshes it.
pub const ACCESS_TOKEN_TTL: i64 = 60 * 60;
/// How long a refresh token lasts without use. Rotation extends the chain, so
/// this only bounds an abandoned connection.
pub const REFRESH_TOKEN_TTL: i64 = 60 * 60 * 24 * 30;

#[derive(Clone)]
pub struct OAuthStore {
    connection: Connection,
}

#[derive(Debug, Clone)]
pub struct Client {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AuthorizationCode {
    pub client_id: String,
    pub user_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub scope: String,
}

#[derive(Debug, Clone)]
pub struct TokenClaims {
    pub client_id: String,
    pub user_id: String,
    pub scope: String,
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl OAuthStore {
    pub async fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)
            .await
            .with_context(|| format!("opening the OAuth store at {}", path.display()))?;
        connection
            .call(|database| {
                database.pragma_update(None, "journal_mode", "WAL")?;
                database.pragma_update(None, "synchronous", "FULL")?;
                database.execute_batch(
                    "CREATE TABLE IF NOT EXISTS clients (
                       client_id TEXT PRIMARY KEY,
                       client_name TEXT NOT NULL,
                       redirect_uris TEXT NOT NULL,
                       created_at INTEGER NOT NULL
                     );
                     CREATE TABLE IF NOT EXISTS codes (
                       code_sha256 BLOB PRIMARY KEY,
                       client_id TEXT NOT NULL,
                       user_id TEXT NOT NULL,
                       redirect_uri TEXT NOT NULL,
                       code_challenge TEXT NOT NULL,
                       scope TEXT NOT NULL,
                       expires_at INTEGER NOT NULL
                     );
                     CREATE TABLE IF NOT EXISTS tokens (
                       token_sha256 BLOB PRIMARY KEY,
                       kind TEXT NOT NULL CHECK (kind IN ('access', 'refresh')),
                       client_id TEXT NOT NULL,
                       user_id TEXT NOT NULL,
                       scope TEXT NOT NULL,
                       expires_at INTEGER NOT NULL
                     );
                     CREATE INDEX IF NOT EXISTS tokens_expiry ON tokens(expires_at);",
                )?;
                Ok(())
            })
            .await?;
        Ok(Self { connection })
    }

    pub async fn register_client(&self, client: Client) -> Result<()> {
        let redirect_uris = serde_json::to_string(&client.redirect_uris)?;
        self.connection
            .call(move |database| {
                database.execute(
                    "INSERT INTO clients(client_id, client_name, redirect_uris, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![client.client_id, client.client_name, redirect_uris, now()],
                )?;
                Ok(())
            })
            .await
            .context("registering an OAuth client")
    }

    pub async fn client(&self, client_id: &str) -> Result<Option<Client>> {
        let client_id = client_id.to_owned();
        let row = self
            .connection
            .call(move |database| {
                database
                    .query_row(
                        "SELECT client_id, client_name, redirect_uris FROM clients
                         WHERE client_id = ?1",
                        [&client_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()
            })
            .await?;
        row.map(|(client_id, client_name, redirect_uris)| {
            Ok(Client {
                client_id,
                client_name,
                redirect_uris: serde_json::from_str(&redirect_uris)?,
            })
        })
        .transpose()
    }

    pub async fn issue_code(&self, code: &str, claims: AuthorizationCode) -> Result<()> {
        let code = digest(code);
        self.connection
            .call(move |database| {
                database.execute(
                    "INSERT INTO codes(code_sha256, client_id, user_id, redirect_uri,
                                       code_challenge, scope, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        code.as_slice(),
                        claims.client_id,
                        claims.user_id,
                        claims.redirect_uri,
                        claims.code_challenge,
                        claims.scope,
                        now() + CODE_TTL,
                    ],
                )?;
                Ok(())
            })
            .await
            .context("issuing an authorization code")
    }

    /// Redeems a code, which can happen at most once: the row is deleted in the
    /// same statement that reads it, so two concurrent exchanges cannot both
    /// succeed.
    pub async fn redeem_code(&self, code: &str) -> Result<Option<AuthorizationCode>> {
        let code = digest(code);
        self.connection
            .call(move |database| {
                let claims = database
                    .query_row(
                        "DELETE FROM codes WHERE code_sha256 = ?1 AND expires_at > ?2
                         RETURNING client_id, user_id, redirect_uri, code_challenge, scope",
                        rusqlite::params![code.as_slice(), now()],
                        |row| {
                            Ok(AuthorizationCode {
                                client_id: row.get(0)?,
                                user_id: row.get(1)?,
                                redirect_uri: row.get(2)?,
                                code_challenge: row.get(3)?,
                                scope: row.get(4)?,
                            })
                        },
                    )
                    .optional()?;
                database.execute("DELETE FROM codes WHERE expires_at <= ?1", [now()])?;
                Ok(claims)
            })
            .await
            .context("redeeming an authorization code")
    }

    pub async fn issue_token(
        &self,
        token: &str,
        kind: &'static str,
        claims: TokenClaims,
    ) -> Result<()> {
        let token = digest(token);
        let ttl = if kind == "access" {
            ACCESS_TOKEN_TTL
        } else {
            REFRESH_TOKEN_TTL
        };
        self.connection
            .call(move |database| {
                database.execute(
                    "INSERT INTO tokens(token_sha256, kind, client_id, user_id, scope, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        token.as_slice(),
                        kind,
                        claims.client_id,
                        claims.user_id,
                        claims.scope,
                        now() + ttl,
                    ],
                )?;
                Ok(())
            })
            .await
            .context("issuing a token")
    }

    /// Looks up a live token of the given kind.
    ///
    /// The digest is fetched by primary key and then compared again in constant
    /// time, so the comparison itself leaks nothing beyond what the index
    /// already does.
    pub async fn token(&self, token: &str, kind: &'static str) -> Result<Option<TokenClaims>> {
        let wanted = digest(token);
        self.connection
            .call(move |database| {
                let row = database
                    .query_row(
                        "SELECT token_sha256, client_id, user_id, scope FROM tokens
                         WHERE token_sha256 = ?1 AND kind = ?2 AND expires_at > ?3",
                        rusqlite::params![wanted.as_slice(), kind, now()],
                        |row| {
                            Ok((
                                row.get::<_, Vec<u8>>(0)?,
                                TokenClaims {
                                    client_id: row.get(1)?,
                                    user_id: row.get(2)?,
                                    scope: row.get(3)?,
                                },
                            ))
                        },
                    )
                    .optional()?;
                Ok(row.and_then(|(stored, claims)| {
                    bool::from(stored.ct_eq(wanted.as_slice())).then_some(claims)
                }))
            })
            .await
            .context("reading a token")
    }

    /// Consumes a refresh token. Rotation means the presented token stops
    /// working the moment a replacement is issued.
    pub async fn revoke_token(&self, token: &str) -> Result<()> {
        let token = digest(token);
        self.connection
            .call(move |database| {
                database.execute(
                    "DELETE FROM tokens WHERE token_sha256 = ?1",
                    [token.as_slice()],
                )?;
                database.execute("DELETE FROM tokens WHERE expires_at <= ?1", [now()])?;
                Ok(())
            })
            .await
            .context("revoking a token")
    }
}
