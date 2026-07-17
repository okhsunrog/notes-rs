//! Typed AI configuration primitives shared by desktop and server hosts.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedConfigValue {
    setting: &'static str,
    value: String,
}

impl fmt::Display for UnsupportedConfigValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unsupported {}: {}", self.setting, self.value)
    }
}

impl std::error::Error for UnsupportedConfigValue {}

macro_rules! config_enum {
    ($name:ident { $($variant:ident => [$primary:literal $(, $alias:literal)*]),+ $(,)? }) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type,
        )]
        #[serde(rename_all = "lowercase")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $primary),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnsupportedConfigValue;

            fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
                match value {
                    $($primary $(| $alias)* => Ok(Self::$variant),)+
                    _ => Err(UnsupportedConfigValue {
                        setting: stringify!($name),
                        value: value.to_owned(),
                    }),
                }
            }
        }
    };
}

config_enum!(CompletionProtocol {
    Openai => ["openai"],
    Anthropic => ["anthropic"],
});
config_enum!(ExtractionProtocol {
    Inherit => ["inherit"],
    Openai => ["openai"],
    Anthropic => ["anthropic"],
});
config_enum!(EmbeddingProvider {
    Openrouter => ["openrouter"],
    Openai => ["openai"],
    Cohere => ["cohere"],
    Voyageai => ["voyageai"],
    Gemini => ["gemini"],
    Local => ["local", "fastembed"],
});
config_enum!(RerankProvider {
    Openrouter => ["openrouter"],
    Local => ["local", "fastembed"],
});

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

pub fn uses_openrouter_endpoint(protocol: CompletionProtocol, base_url: Option<&str>) -> bool {
    if protocol != CompletionProtocol::Openai {
        return false;
    }
    let host = base_url
        .and_then(|value| reqwest::Url::parse(value).ok())
        .and_then(|url| url.host_str().map(str::to_owned));
    host.as_deref() == Some("openrouter.ai")
}

pub fn completion_config(
    protocol: CompletionProtocol,
    base_url: Option<String>,
    api_key: Option<String>,
    model: String,
) -> Result<llm_relay::ClientConfig> {
    let api_key = api_key.unwrap_or_default();
    match protocol {
        CompletionProtocol::Openai => {
            let base_url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".into());
            let config = llm_relay::ClientConfig::openai_compatible(base_url, api_key, model);
            if config.api_key.is_empty() {
                Ok(config.without_auth())
            } else {
                Ok(config)
            }
        }
        CompletionProtocol::Anthropic => Ok(llm_relay::ClientConfig::anthropic(api_key, model)
            .base_url(base_url.unwrap_or_else(|| "https://api.anthropic.com".into()))),
    }
}

impl From<ExtractionProtocol> for CompletionProtocol {
    fn from(value: ExtractionProtocol) -> Self {
        match value {
            ExtractionProtocol::Openai => Self::Openai,
            ExtractionProtocol::Anthropic => Self::Anthropic,
            ExtractionProtocol::Inherit => {
                unreachable!("inherit is resolved before selecting a completion transport")
            }
        }
    }
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
            CompletionProtocol::Openai,
            Some("http://localhost:11434/v1".into()),
            None,
            "qwen3".into(),
        )
        .expect("local OpenAI-compatible config");
        assert_eq!(openai.provider, llm_relay::Provider::OpenAiCompatible);
        assert_eq!(openai.auth_scheme, llm_relay::AuthScheme::None);

        let anthropic = completion_config(
            CompletionProtocol::Anthropic,
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
