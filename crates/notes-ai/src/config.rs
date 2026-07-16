//! Environment-backed AI configuration shared by desktop and server hosts.

use anyhow::{Context, Result, bail};

pub const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

pub fn validate_http_base_url(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value.trim()).context("base URL must be a valid URL")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("base URL must use http or https and include a host");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("base URL cannot contain a query string or fragment");
    }
    Ok(())
}

pub fn openrouter_client() -> Result<rig::providers::openrouter::Client> {
    let api_key = std::env::var("OPENROUTER_API_KEY").context("OPENROUTER_API_KEY not set")?;
    let base_url =
        std::env::var("OPENROUTER_BASE_URL").unwrap_or_else(|_| DEFAULT_OPENROUTER_BASE_URL.into());
    openrouter_client_from(api_key, &base_url)
}

pub fn openrouter_client_from(
    api_key: String,
    base_url: &str,
) -> Result<rig::providers::openrouter::Client> {
    validate_http_base_url(base_url)?;
    rig::providers::openrouter::Client::builder()
        .base_url(base_url.trim_end_matches('/'))
        .api_key(api_key)
        .build()
        .context("building OpenRouter client")
}

fn env_flag(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| value.eq_ignore_ascii_case("true") || value == "1")
        .unwrap_or(default)
}

pub fn local_only() -> bool {
    env_flag("AI_LOCAL_ONLY", false)
}

pub fn entity_extraction_enabled() -> bool {
    env_flag("ENTITY_EXTRACTION_ENABLED", true) && !local_only()
}

pub fn query_rewriting_enabled() -> bool {
    env_flag("QUERY_REWRITING_ENABLED", true) && !local_only()
}

pub fn chat_model() -> String {
    std::env::var("CHAT_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4-flash".into())
}

pub fn extraction_model() -> String {
    std::env::var("EXTRACT_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4-flash".into())
}

pub fn chat_completion_config() -> Result<llm_relay::ClientConfig> {
    let protocol = std::env::var("CHAT_PROTOCOL").unwrap_or_else(|_| "openai".into());
    let base_url = std::env::var("CHAT_BASE_URL")
        .ok()
        .or_else(|| std::env::var("OPENROUTER_BASE_URL").ok());
    let api_key = std::env::var("CHAT_API_KEY")
        .ok()
        .or_else(|| legacy_openrouter_key(&protocol, base_url.as_deref()));
    completion_config(&protocol, base_url, api_key, chat_model())
}

pub fn extraction_completion_config() -> Result<llm_relay::ClientConfig> {
    let protocol = std::env::var("EXTRACT_PROTOCOL").unwrap_or_else(|_| "inherit".into());
    if protocol == "inherit" {
        let mut config = chat_completion_config()?;
        config.model = extraction_model();
        return Ok(config);
    }
    let base_url = std::env::var("EXTRACT_BASE_URL").ok();
    let api_key = std::env::var("EXTRACT_API_KEY")
        .ok()
        .or_else(|| std::env::var("CHAT_API_KEY").ok())
        .or_else(|| legacy_openrouter_key(&protocol, base_url.as_deref()));
    completion_config(&protocol, base_url, api_key, extraction_model())
}

pub fn legacy_openrouter_key(protocol: &str, base_url: Option<&str>) -> Option<String> {
    if protocol != "openai" {
        return None;
    }
    let host = base_url
        .and_then(|value| reqwest::Url::parse(value).ok())
        .and_then(|url| url.host_str().map(str::to_owned));
    if host.as_deref() == Some("openrouter.ai") {
        std::env::var("OPENROUTER_API_KEY").ok()
    } else {
        None
    }
}

pub fn completion_config(
    protocol: &str,
    base_url: Option<String>,
    api_key: Option<String>,
    model: String,
) -> Result<llm_relay::ClientConfig> {
    let api_key = api_key.unwrap_or_default();
    match protocol {
        "openai" => {
            let base_url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".into());
            let config = llm_relay::ClientConfig::openai_compatible(base_url, api_key, model);
            if config.api_key.is_empty() {
                Ok(config.without_auth())
            } else {
                Ok(config)
            }
        }
        "anthropic" => Ok(llm_relay::ClientConfig::anthropic(api_key, model)
            .base_url(base_url.unwrap_or_else(|| "https://api.anthropic.com".into()))),
        other => bail!("unsupported completion protocol: {other}"),
    }
}

pub fn ensure_cloud_ai_allowed(feature: &str) -> Result<()> {
    if local_only() {
        bail!("{feature} is disabled in local-only mode because no local chat model is configured")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_provider_base_urls() {
        assert!(validate_http_base_url("https://proxy.example/v1/").is_ok());
        assert!(validate_http_base_url("http://127.0.0.1:11434/v1").is_ok());
        assert!(validate_http_base_url("file:///tmp/api").is_err());
        assert!(validate_http_base_url("https://proxy.example/v1?token=secret").is_err());
    }

    #[test]
    fn openrouter_client_honors_custom_base_url() {
        let client = openrouter_client_from("test-key".into(), "http://127.0.0.1:11434/custom/v1/")
            .expect("build custom client");
        assert_eq!(client.base_url(), "http://127.0.0.1:11434/custom/v1");
    }

    #[test]
    fn builds_protocol_neutral_completion_configs() {
        let openai = completion_config(
            "openai",
            Some("http://localhost:11434/v1".into()),
            None,
            "qwen3".into(),
        )
        .expect("local OpenAI-compatible config");
        assert_eq!(openai.provider, llm_relay::Provider::OpenAiCompatible);
        assert_eq!(openai.auth_scheme, llm_relay::AuthScheme::None);

        let anthropic = completion_config(
            "anthropic",
            Some("https://proxy.example/anthropic".into()),
            Some("secret".into()),
            "custom-claude".into(),
        )
        .expect("Anthropic-compatible config");
        assert_eq!(anthropic.provider, llm_relay::Provider::Anthropic);
        assert_eq!(
            anthropic.auth_scheme,
            llm_relay::AuthScheme::Header("x-api-key".into())
        );
    }
}
