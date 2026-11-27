//! Ensures management routes never reach a network listener or upstream proxy.
use super::router_for_listener;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{any, get},
    Router,
};
use tower::ServiceExt;

#[tokio::test]
async fn management_routes_require_an_actual_loopback_listener() {
    for (address, expected) in [
        ("127.0.0.1:3000", StatusCode::NO_CONTENT),
        ("[::1]:3000", StatusCode::NO_CONTENT),
        ("0.0.0.0:3000", StatusCode::FORBIDDEN),
        ("[::]:3000", StatusCode::FORBIDDEN),
        ("192.168.1.2:3000", StatusCode::FORBIDDEN),
    ] {
        let router = Router::new()
            .route(
                "/byok/v1/models",
                get(|| async { StatusCode::UNAUTHORIZED }),
            )
            .route(
                "/__byok-api__/healthz",
                get(|| async { StatusCode::NO_CONTENT }),
            )
            .fallback(|| async { StatusCode::BAD_GATEWAY });
        let mut control = Router::new();
        let management_paths = [
            "/__byok-api__",
            "/__byok-api__/",
            "/__byok-api__/api/settings/compaction",
            "/__byok-api__/api/desktop/open-external-url",
            "/__byok-api__/assets/index.js",
        ];
        for path in management_paths {
            control = control.route(path, any(|| async { StatusCode::NO_CONTENT }));
        }
        let router = router_for_listener(router, control, address.parse().unwrap());
        for path in management_paths {
            for method in ["GET", "PUT", "POST", "DELETE"] {
                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri(path)
                            .method(method)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), expected, "{address} {method} {path}");
            }
        }
        for (path, status) in [
            ("/byok/v1/models", StatusCode::UNAUTHORIZED),
            ("/__byok-api__/healthz", StatusCode::NO_CONTENT),
        ] {
            let response = router
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{address} {path}");
        }
    }
}
