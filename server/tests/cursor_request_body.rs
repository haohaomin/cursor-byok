//! Bound Cursor unary request buffering before protobuf parsing or dispatch.
#[path = "support/fake_provider.rs"]
mod fake_provider;
#[path = "support/fixtures.rs"]
mod fixtures;

use std::{
    convert::Infallible,
    io::Write,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, Request, StatusCode},
    Router,
};
use cursor_server::{
    api::cursor,
    cursor::{
        prompting::{PromptAssets, PromptCompiler},
        transport::TransportRegistry,
    },
    network::NetworkClients,
};
use flate2::{write::GzEncoder, Compression};
use futures_util::stream;
use tower::ServiceExt;

const LIMIT: usize = 64 * 1024 * 1024;
const BIDI: &str = "/aiserver.v1.BidiService/BidiAppend";
const PROBE: &str = "/aiserver.v1.NetworkService/IsConnected";

async fn test_router() -> (
    tempfile::TempDir,
    Router,
    TransportRegistry,
    fake_provider::FakeProvider,
) {
    let (directory, store) = fixtures::temp_store().await;
    store
        .set_tab_settings(cursor_server::store::TabSettings {
            mode: cursor_server::store::TabMode::Direct,
            address: String::new(),
        })
        .await
        .unwrap();
    let provider = fake_provider::FakeProvider::default();
    let assets =
        PromptAssets::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor"))
            .unwrap();
    let registry = TransportRegistry::new(
        store.clone(),
        Arc::new(provider.clone()),
        PromptCompiler::new(assets),
    );
    let router = cursor::router(registry.clone(), NetworkClients::new(store)).unwrap();
    (directory, router, registry, provider)
}

async fn assert_too_large(response: axum::response::Response) {
    // Connect maps resource_exhausted to HTTP 429, including request size limits.
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["code"], "resource_exhausted");
    assert!(error["message"].as_str().unwrap().contains("64 MiB"));
}

#[tokio::test]
async fn advertised_oversize_is_rejected_without_reading_or_dispatching() {
    let (_directory, router, registry, provider) = test_router().await;
    let body = Body::from_stream(stream::pending::<Result<Bytes, Infallible>>());
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        router.oneshot(
            Request::post(BIDI)
                .header(header::CONTENT_LENGTH, LIMIT + 1)
                .body(body)
                .unwrap(),
        ),
    )
    .await
    .expect("must reject before waiting for the body")
    .unwrap();
    assert_too_large(response).await;
    assert!(provider.requests().is_empty());
    registry.shutdown().await;
}

#[tokio::test]
async fn chunked_oversize_stops_reading_at_the_limit() {
    let (_directory, router, registry, provider) = test_router().await;
    let polls = Arc::new(AtomicUsize::new(0));
    let observed = polls.clone();
    let chunk = Bytes::from(vec![0; 1024 * 1024]);
    let body = Body::from_stream(stream::iter((0..128).map(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok::<_, Infallible>(chunk.clone())
    })));
    assert_too_large(
        router
            .oneshot(Request::post(BIDI).body(body).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(polls.load(Ordering::SeqCst), 65);
    assert!(provider.requests().is_empty());
    registry.shutdown().await;
}

#[tokio::test]
async fn gzip_expansion_is_bounded_before_protobuf_decode() {
    let (_directory, router, registry, provider) = test_router().await;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let chunk = vec![0; 1024 * 1024];
    for _ in 0..64 {
        encoder.write_all(&chunk).unwrap();
    }
    encoder.write_all(&[0]).unwrap();
    let compressed = encoder.finish().unwrap();
    assert!(compressed.len() < 1024 * 1024);
    let response = router
        .oneshot(
            Request::post(BIDI)
                .header(header::CONTENT_ENCODING, "gzip")
                .header(header::CONTENT_LENGTH, compressed.len())
                .body(Body::from(compressed))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_too_large(response).await;
    assert!(provider.requests().is_empty());
    registry.shutdown().await;
}

#[tokio::test]
async fn exact_limit_is_accepted_but_one_more_byte_is_rejected() {
    let (_directory, router, registry, _) = test_router().await;
    for (size, accepted) in [(LIMIT, true), (LIMIT + 1, false)] {
        let body = Body::from(vec![0; size]);
        let response = router
            .clone()
            .oneshot(Request::post(BIDI).body(body).unwrap())
            .await
            .unwrap();
        if accepted {
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            assert!(String::from_utf8_lossy(&body).contains("protobuf decode error"));
        } else {
            assert_too_large(response).await;
        }
    }
    registry.shutdown().await;
}

#[tokio::test]
async fn malformed_and_unsupported_compression_remain_distinct_errors() {
    let (_directory, router, registry, _) = test_router().await;
    for (encoding, status) in [
        ("gzip", StatusCode::BAD_REQUEST),
        ("byok-unsupported", StatusCode::UNSUPPORTED_MEDIA_TYPE),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::post(BIDI)
                    .header(header::CONTENT_ENCODING, encoding)
                    .body(Body::from("not compressed data"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    registry.shutdown().await;
}

#[tokio::test]
async fn tab_and_unknown_routes_do_not_buffer_uploads() {
    let (_directory, router, registry, _) = test_router().await;
    for (path, method) in [
        ("/aiserver.v1.AiService/StreamCpp", axum::http::Method::POST),
        (
            "/aiserver.v1.FileSyncService/FSUploadFile",
            axum::http::Method::POST,
        ),
        ("/unknown.Service/Stream", axum::http::Method::POST),
        (BIDI, axum::http::Method::GET),
    ] {
        let body = Body::from_stream(stream::pending::<Result<Bytes, Infallible>>());
        // Upstream validation precedes upload polling in the streaming relay.
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            router.clone().oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(cursor::proxy::UPSTREAM_URL_HEADER, "http://127.0.0.1:1")
                    .header(header::CONTENT_LENGTH, LIMIT + 1)
                    .body(body)
                    .unwrap(),
            ),
        )
        .await
        .expect("streaming routes must reach the relay before upload ends")
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Cursor HTTPS host"));
    }
    registry.shutdown().await;
}

#[tokio::test]
async fn oversized_compressed_wire_is_rejected_before_decoding() {
    let (_directory, router, registry, _) = test_router().await;
    // A dishonest length must not bypass the actual wire read limit. If layers
    // are reversed, invalid gzip is reported instead of the size error.
    let response = router
        .oneshot(
            Request::post(BIDI)
                .header(header::CONTENT_ENCODING, "gzip")
                .header(header::CONTENT_LENGTH, 1)
                .body(Body::from(vec![0; LIMIT + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_too_large(response).await;
    registry.shutdown().await;
}

#[tokio::test]
async fn bodyless_probes_do_not_wait_for_uploads() {
    let (_directory, router, registry, _) = test_router().await;
    for (path, method, status) in [
        (
            "/__byok-api__/healthz",
            axum::http::Method::GET,
            StatusCode::NO_CONTENT,
        ),
        (PROBE, axum::http::Method::POST, StatusCode::OK),
    ] {
        let body = Body::from_stream(stream::pending::<Result<Bytes, Infallible>>());
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            router.clone().oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(body)
                    .unwrap(),
            ),
        )
        .await
        .expect("bodyless probes must not wait for upload completion")
        .unwrap();
        assert_eq!(response.status(), status);
    }
    registry.shutdown().await;
}

#[tokio::test]
async fn tab_and_file_uploads_stream_through_the_full_cursor_router() {
    use futures_util::StreamExt;
    use tokio_stream::wrappers::ReceiverStream;
    let (_directory, router, registry, _) = test_router().await;
    registry
        .store()
        .set_proxy_settings(cursor_server::store::ProxySettingsInput {
            mode: cursor_server::store::ProxyMode::Custom,
            address: "http://127.0.0.1:1".into(),
            auth_enabled: false,
            username: String::new(),
            password: None,
        })
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    registry
        .store()
        .set_tab_settings(cursor_server::store::TabSettings {
            mode: cursor_server::store::TabMode::Custom,
            address: format!("http://{}", listener.local_addr().unwrap()),
        })
        .await
        .unwrap();
    let upstream = Router::new().fallback(|request: Request<Body>| async {
        Body::from_stream(request.into_body().into_data_stream())
    });
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        for path in [
            "/aiserver.v1.AiService/StreamCpp",
            "/aiserver.v1.FileSyncService/FSUploadFile",
        ] {
            let (upload, chunks) = tokio::sync::mpsc::channel::<Result<Bytes, Infallible>>(2);
            upload.send(Ok(Bytes::from_static(b"first"))).await.unwrap();
            let response = router
                .clone()
                .oneshot(
                    Request::post(path)
                        .body(Body::from_stream(ReceiverStream::new(chunks)))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut chunks = response.into_body().into_data_stream();
            assert_eq!(chunks.next().await.unwrap().unwrap(), "first");
            upload
                .send(Ok(Bytes::from_static(b"second")))
                .await
                .unwrap();
            assert_eq!(chunks.next().await.unwrap().unwrap(), "second");
            drop(upload);
            while let Some(chunk) = chunks.next().await {
                assert!(chunk.unwrap().is_empty());
            }
        }
    })
    .await;
    server.abort();
    registry.shutdown().await;
    result.expect("relay must return each response chunk while the upload is still open");
}
