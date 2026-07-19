use notes_protocol::{
    AiProviderSettingsUpdate, ApiErrorCode, ApiErrorDetail, ApiErrorResponse, CompletionProtocol,
    SearchRequest, ServerErrorCode, ServerMessage,
};

#[test]
fn error_codes_use_stable_snake_case_wire_values() {
    let server_codes = [
        (ServerErrorCode::CatchUpFailed, "catch_up_failed"),
        (ServerErrorCode::UnsupportedMessage, "unsupported_message"),
        (ServerErrorCode::IngestFailed, "ingest_failed"),
        (ServerErrorCode::BatchTooLarge, "batch_too_large"),
        (ServerErrorCode::InvalidMessage, "invalid_message"),
        (ServerErrorCode::ResyncRequired, "resync_required"),
        (ServerErrorCode::Conflict, "conflict"),
    ];
    for (code, expected) in server_codes {
        assert_eq!(
            serde_json::to_value(code).expect("server error code"),
            expected
        );
        assert_eq!(
            serde_json::from_value::<ServerErrorCode>(serde_json::json!(expected))
                .expect("decode server error code"),
            code
        );
        assert_eq!(code.as_str(), expected);
        assert_eq!(code.to_string(), expected);
    }

    let api_codes = [
        (ApiErrorCode::InvalidRequest, "invalid_request"),
        (ApiErrorCode::Unauthorized, "unauthorized"),
        (ApiErrorCode::NotFound, "not_found"),
        (ApiErrorCode::Conflict, "conflict"),
        (ApiErrorCode::PayloadTooLarge, "payload_too_large"),
        (ApiErrorCode::Unavailable, "unavailable"),
        (ApiErrorCode::Internal, "internal"),
    ];
    for (code, expected) in api_codes {
        assert_eq!(
            serde_json::to_value(code).expect("API error code"),
            expected
        );
        assert_eq!(
            serde_json::from_value::<ApiErrorCode>(serde_json::json!(expected))
                .expect("decode API error code"),
            code
        );
    }
}

#[test]
fn legacy_search_requests_default_to_reranking() {
    let request: SearchRequest = serde_json::from_value(serde_json::json!({
        "query": "project",
        "limit": 8
    }))
    .expect("decode legacy search request");
    assert!(request.rerank);

    let without_reranking: SearchRequest = serde_json::from_value(serde_json::json!({
        "query": "project",
        "limit": 8,
        "rerank": false
    }))
    .expect("decode explicit rerank preference");
    assert!(!without_reranking.rerank);
}

#[test]
fn structured_errors_round_trip_without_stringly_typed_codes() {
    let websocket = ServerMessage::Error {
        code: ServerErrorCode::Conflict,
        message: "operation identity was reused".into(),
    };
    let websocket_json = serde_json::to_value(&websocket).expect("websocket error");
    assert_eq!(
        websocket_json,
        serde_json::json!({
            "type": "error",
            "code": "conflict",
            "message": "operation identity was reused"
        })
    );
    assert_eq!(
        serde_json::from_value::<ServerMessage>(websocket_json).expect("websocket error decode"),
        websocket
    );

    let http = ApiErrorResponse {
        error: ApiErrorDetail {
            code: ApiErrorCode::PayloadTooLarge,
            message: "blob is too large".into(),
        },
    };
    let http_json = serde_json::to_value(&http).expect("HTTP error");
    assert_eq!(http_json["error"]["code"], "payload_too_large");
    assert_eq!(
        serde_json::from_value::<ApiErrorResponse>(http_json).expect("HTTP error decode"),
        http
    );
}

fn provider_update_json(retrieval_base_url: &str, completion_base_url: &str) -> serde_json::Value {
    serde_json::json!({
        "retrievalBaseUrl": retrieval_base_url,
        "retrievalApiKey": null,
        "embeddingModel": "embedding-model",
        "embeddingDimensions": 1024,
        "rerankModel": "rerank-model",
        "completionProtocol": "openai",
        "completionBaseUrl": completion_base_url,
        "completionApiKey": null,
        "chatModel": "chat-model",
        "extractionModel": "extraction-model"
    })
}

#[test]
fn provider_urls_are_parsed_at_the_wire_boundary() {
    let update: AiProviderSettingsUpdate = serde_json::from_value(provider_update_json(
        "https://retrieval.example.test/v1",
        "https://completion.example.test/v1",
    ))
    .expect("valid provider update");
    assert_eq!(update.retrieval_base_url.scheme(), "https");
    assert_eq!(
        update.completion_base_url.host_str(),
        Some("completion.example.test")
    );
    assert_eq!(update.completion_protocol, CompletionProtocol::Openai);

    for invalid in ["not a URL", "/relative/provider", "https://[invalid"] {
        assert!(
            serde_json::from_value::<AiProviderSettingsUpdate>(provider_update_json(
                invalid,
                "https://completion.example.test/v1"
            ))
            .is_err(),
            "{invalid:?} must not deserialize as a provider URL"
        );
        assert!(
            serde_json::from_value::<AiProviderSettingsUpdate>(provider_update_json(
                "https://retrieval.example.test/v1",
                invalid
            ))
            .is_err(),
            "{invalid:?} must not deserialize as a provider URL"
        );
    }
}
