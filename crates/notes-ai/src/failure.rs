pub(crate) fn provider_failure_is_terminal(error: &anyhow::Error) -> bool {
    for cause in error.chain() {
        if let Some(request) = cause.downcast_ref::<reqwest::Error>() {
            return request_is_terminal(request);
        }
        if let Some(relay) = cause.downcast_ref::<llm_relay::LlmError>() {
            return match relay {
                llm_relay::LlmError::ApiError { status, .. } => status_is_terminal(*status),
                llm_relay::LlmError::Request(request) => request_is_terminal(request),
                llm_relay::LlmError::Config(_)
                | llm_relay::LlmError::Client(_)
                | llm_relay::LlmError::ResponseTooLarge { .. } => true,
                llm_relay::LlmError::InvalidStructuredOutput { .. }
                | llm_relay::LlmError::ParseResponse(_)
                | llm_relay::LlmError::EmptyResponse
                | llm_relay::LlmError::Conversion(_)
                | llm_relay::LlmError::Stream(_) => false,
            };
        }
    }
    false
}

fn request_is_terminal(error: &reqwest::Error) -> bool {
    if error.is_timeout() || error.is_connect() {
        return false;
    }
    error
        .status()
        .is_some_and(|status| status_is_terminal(status.as_u16()))
}

fn status_is_terminal(status: u16) -> bool {
    !matches!(status, 408 | 429 | 500..=599)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_policy_distinguishes_transient_http_statuses() {
        for status in [408, 429, 500, 503] {
            assert!(!status_is_terminal(status));
        }
        for status in [400, 401, 403, 404] {
            assert!(status_is_terminal(status));
        }
    }
}
