//! Tests model connectivity through the control service and actual Responses HTTP/SSE.
#[path = "support/fixtures.rs"]
mod fixtures;

use axum::{http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use cursor_server::{
    control::ControlService,
    model::{ModelConfigInput, ModelType, OPENAI_RESPONSES_ENDPOINT},
    network::NetworkClients,
    plugin::{PluginRegistry, PluginRuntime},
    provider::ProviderRouter,
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

fn model_input() -> ModelConfigInput {
    ModelConfigInput {
        sort_order: 0,
        display_name: "Example".into(),
        group_name: Some("work".into()),
        model_type: ModelType::OpenAi,
        base_url: "https://example.com/v1".into(),
        use_full_url: false,
        api_key: "upstream".into(),
        tooltip_data: "test".into(),
        model_id: "qwen/model".into(),
        reasoning_effort: None,
        openai_endpoint: OPENAI_RESPONSES_ENDPOINT.into(),
        openai_extra_params_enabled: false,
        openai_extra_params: json!({}),
        custom_headers_enabled: false,
        strip_images: false,
        custom_headers: json!({}),
        anthropic_extra_params_enabled: false,
        anthropic_extra_params: json!({}),
        context_window_tokens: None,
        max_completion_tokens: None,
        anthropic_max_tokens: None,
        anthropic_thinking_effort: None,
        thinking_budget_tokens: None,
    }
}

#[tokio::test]
async fn connectivity_accepts_provider_requiring_nonempty_instructions() {
    check_connectivity(true).await;
}

#[tokio::test]
async fn connectivity_accepts_provider_allowing_empty_instructions() {
    check_connectivity(false).await;
}

async fn check_connectivity(require_instructions: bool) {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests = captured.clone();
    let output = (1..=120)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let expected = output.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route("/v1/responses", post(move |Json(body): Json<Value>| {
        let requests = requests.clone();
        let output = output.clone();
        async move {
            requests.lock().unwrap().push(body.clone());
            if require_instructions && body["instructions"].as_str().is_none_or(|s| s.trim().is_empty()) {
                return (StatusCode::BAD_REQUEST, Json(json!({"detail":"Instructions are required"}))).into_response();
            }
            let delta = json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":output});
            let done = json!({"type":"response.completed","response":{"output":[],"usage":{"input_tokens":25,"output_tokens":120}}});
            ([("content-type", "text/event-stream")], format!("event: response.output_text.delta\ndata: {delta}\n\nevent: response.completed\ndata: {done}\n\n")).into_response()
        }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (_directory, store) = fixtures::temp_store().await;
    let mut input = model_input();
    input.base_url = format!("http://{address}/v1");
    let model = store.create_model(&input).await.unwrap();
    let runtime = PluginRuntime::managed().unwrap();
    let plugins = PluginRegistry::managed(store.clone(), runtime.clone(), "0.1.0".into()).unwrap();
    let clients = NetworkClients::new(store.clone());
    let provider = Arc::new(ProviderRouter::new(
        store.clone(),
        plugins.clone(),
        clients.clone(),
        Duration::from_secs(5),
        Duration::from_secs(5),
    ));
    let control = ControlService::new(store.clone(), provider, runtime, plugins, clients).unwrap();
    let result = control
        .test_model(&model.model_hash, "connectivity-contract")
        .await;
    server.abort();
    let result = result.expect("connectivity request should be accepted and finish successfully");
    assert_eq!(result.output, expected);
    assert_eq!(result.output_tokens, 120);
    assert!(!result.tokens_estimated);
    assert!(result.first_valid_response_ms.is_some());
    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request["model"], input.model_id);
    assert_eq!(request["stream"], true);
    assert!(request.get("tools").is_none());
    let messages = request["input"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"][0]["text"], "Output the numbers 1 through 120 separated by a single space. No commas, no newlines, no explanation.");
}
