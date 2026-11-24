//! Team metadata is empty only for the local BYOK identity.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use axum::{body::Bytes, http::Method};
use tower::ServiceExt;

use super::*;
use crate::{
    cursor::prompting::{PromptAssets, PromptCompiler},
    model::ModelInvocation,
    network::NetworkClients,
    provider::{Provider, ProviderStream},
    store::Store,
};

const PATHS: [&str; 2] = [
    "/aiserver.v1.DashboardService/GetTeamReposOrEmptyIfNotInTeam",
    "/aiserver.v1.DashboardService/GetTeamAdminSettingsOrEmptyIfNotInTeam",
];

struct NoProvider;
impl Provider for NoProvider {
    fn stream(&self, _: ModelInvocation, _: tokio_util::sync::CancellationToken) -> ProviderStream {
        panic!("team metadata must not invoke a model")
    }
}

async fn test_router() -> (tempfile::TempDir, Router) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let clients = NetworkClients::new(store.clone());
    let assets =
        PromptAssets::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor"))
            .unwrap();
    let registry = TransportRegistry::new(store, Arc::new(NoProvider), PromptCompiler::new(assets));
    let knowledge = knowledge::KnowledgeService::with_root(directory.path().join("rules")).unwrap();
    (
        directory,
        router_with_proxy(registry, CursorProxy::cursor(clients), knowledge),
    )
}

fn request(path: &str, method: Method, authorization: Option<&str>, body: Body) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/proto")
        // Makes forwarding observable without contacting a network or real account.
        .header(proxy::UPSTREAM_URL_HEADER, "http://127.0.0.1:1");
    if let Some(authorization) = authorization {
        request = request.header(header::AUTHORIZATION, authorization);
    }
    request.body(body).unwrap()
}

#[tokio::test]
async fn local_team_configuration_consumes_body_and_returns_empty_protobuf() {
    let (_directory, router) = test_router().await;
    for path in PATHS {
        let consumed = Arc::new(AtomicBool::new(false));
        let observed = consumed.clone();
        let body = Body::from_stream(futures_util::stream::once(async move {
            observed.store(true, Ordering::SeqCst);
            Ok::<_, std::convert::Infallible>(Bytes::from_static(b"\x08\x00"))
        }));
        let response = router
            .clone()
            .oneshot(request(
                path,
                Method::POST,
                Some(&crate::local_app::local_cursor_authorization()),
                body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(consumed.load(Ordering::SeqCst));
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/proto"
        );
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "0");
        assert!(to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn other_identities_and_routes_still_use_official_dispatch() {
    let (_directory, router) = test_router().await;
    let local = crate::local_app::local_cursor_authorization();
    let altered = format!("{local}-altered");
    for path in PATHS {
        for (method, authorization) in [
            (Method::POST, Some("Bearer official-test")),
            (Method::POST, Some(altered.as_str())),
            (Method::POST, None),
            (Method::GET, Some(local.as_str())),
        ] {
            let response = router
                .clone()
                .oneshot(request(path, method, authorization, Body::empty()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
            let body = to_bytes(response.into_body(), 8192).await.unwrap();
            assert!(String::from_utf8_lossy(&body)
                .contains("upstream URL must target a Cursor HTTPS host"));
        }
    }
    let response = router
        .oneshot(request(
            "/aiserver.v1.DashboardService/GetTeamAdminSettings",
            Method::POST,
            Some(&local),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "unselected team endpoint must remain upstream"
    );
}
