//! Checks MCP object schemas at the compiler and Responses HTTP boundary.
use super::*;
use crate::{
    config::{ProviderConfig, ProviderKind},
    cursor::prompting::{Mode, PromptAssets, PromptCompiler},
    model::{ModelInvocation, ModelRequest, ModelSpec},
    provider::{ModelEvent, OpenAiResponsesProvider, Provider},
};
use axum::{http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use futures_util::StreamExt;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn responses_accepts_mcp_object_without_properties() {
    assert_responses_accepts(json!({"type":"object"})).await;
}

#[tokio::test]
async fn responses_accepts_empty_mcp_schema() {
    assert_responses_accepts(json!({})).await;
}

async fn assert_responses_accepts(schema: Value) {
    let wire = pb::McpToolDefinition {
        name: "mcp__local__status".into(),
        description: "Get local status".into(),
        input_schema_json: Some(schema.to_string()),
        ..Default::default()
    };
    let context = pb::RequestContext {
        tools: vec![wire.clone()],
        ..Default::default()
    };
    let compiled = dynamic_mcp(&pb::AgentRunRequest::default(), &context).unwrap();
    let (original, definition) = compiled.values().next().unwrap();
    assert_eq!(
        original, &wire,
        "client dispatch schema must remain untouched"
    );
    let assets =
        PromptAssets::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor")).unwrap();
    let compiler = PromptCompiler::new(assets);
    let model = ModelSpec::new("test-model");
    let prompt = compiler
        .prompt_spec(Mode::Ask, &model, std::slice::from_ref(definition), false)
        .unwrap();
    let captured = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let requests = captured.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route("/v1/responses", post(move |Json(body): Json<Value>| {
        let requests = requests.clone();
        async move {
            requests.lock().unwrap().push(body.clone());
            // LM Studio's public llmToolParametersSchema requires an object
            // type and properties record; required is optional. This is a
            // focused contract double, not an actual LM Studio instance.
            for (index, tool) in body["tools"].as_array().unwrap().iter().enumerate() {
                if tool["parameters"]["type"] != "object"
                    || !tool["parameters"]["properties"].is_object()
                {
                    return (StatusCode::BAD_REQUEST, Json(json!({"error":{
                        "message":"Invalid input", "type":"invalid_request_error",
                        "param":format!("tools.{index}"), "code":"custom"
                    }}))).into_response();
                }
            }
            ([("content-type", "text/event-stream")],
             "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n")
                .into_response()
        }
    }));
    let shutdown = CancellationToken::new();
    let stopped = shutdown.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
            .unwrap();
    });
    let provider = OpenAiResponsesProvider::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        ProviderConfig {
            kind: ProviderKind::OpenAiResponses,
            request_url: format!("http://{address}/v1/responses"),
            api_key: "test-only".into(),
            custom_headers: Default::default(),
            max_output_tokens: None,
            request_timeout: Duration::from_secs(5),
            allowed_body_fields: None,
        },
    );
    let invocation = ModelInvocation {
        call_id: "schema-call".into(),
        run_id: "schema-run".into(),
        conversation_id: "schema-conversation".into(),
        provider_call_index: 0,
        request: ModelRequest {
            prompt,
            model,
            history: vec![],
        },
    };
    let results = tokio::time::timeout(
        Duration::from_secs(5),
        provider
            .stream(invocation.clone(), CancellationToken::new())
            .collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    let retry = tokio::time::timeout(
        Duration::from_secs(5),
        provider
            .stream(invocation, CancellationToken::new())
            .collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    shutdown.cancel();
    server.await.unwrap();
    for events in [&results, &retry] {
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert!(events
            .iter()
            .any(|event| matches!(event, Ok(ModelEvent::Done(_)))));
    }
    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0], requests[1],
        "retry must preserve tool definitions"
    );
    let parameters = &requests[0]["tools"].as_array().unwrap().last().unwrap()["parameters"];
    assert_eq!(parameters, &json!({"type":"object","properties":{}}));
}

#[test]
fn mcp_object_union_preserves_branches_and_constraints() {
    let original = json!({"oneOf":[
        {"type":"object","properties":{"a":{"type":"string"}},"required":["a"],"additionalProperties":false},
        {"type":"object","properties":{"b":{"type":"integer"}},"required":["b"],"additionalProperties":false}
    ]});
    let normalized = normalize_mcp_parameters("union", original.clone()).unwrap();
    assert_eq!(normalized["type"], "object");
    assert_eq!(normalized["properties"], json!({}));
    assert_eq!(normalized["oneOf"], original["oneOf"]);
    assert_eq!(
        normalize_mcp_parameters("union", normalized.clone()).unwrap(),
        normalized
    );
}

#[test]
fn mcp_explicit_object_constraints_are_not_rewritten() {
    let schema = json!({"type":"object", "properties":{"id":{"type":"string"}},
        "required":["id"], "additionalProperties":false, "$defs":{"value":{"type":"number"}}});
    assert_eq!(
        normalize_mcp_parameters("valid", schema.clone()).unwrap(),
        schema
    );
    for invalid in [
        json!(false),
        json!({"type":"string"}),
        json!({"type":"object","properties":[]}),
        json!({"anyOf":[{"type":"object"},{"type":"string"}]}),
    ] {
        assert!(
            normalize_mcp_parameters("invalid", invalid.clone()).is_err(),
            "{invalid}"
        );
    }
}
