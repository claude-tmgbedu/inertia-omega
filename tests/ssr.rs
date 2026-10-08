//! The HTTP SSR gateway, against a local SSR server.

#![cfg(feature = "ssr")]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use http::header::{HeaderName, HeaderValue};
use http::{Method, header};
use inertia::ssr::{Gateway, HttpGateway, Rendered, SsrErrorType, SsrFailure};
use inertia::{Page, Request};
use serde_json::{Value, json};

/// Serve `app` on a free local port.
async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    format!("http://{addr}")
}

/// A URL nothing listens on.
async fn closed_url() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    drop(listener);

    format!("http://{addr}")
}

/// An SSR server that renders the page component, counting renders.
async fn renderer(renders: Arc<AtomicUsize>) -> String {
    serve(Router::new().route(
        "/render",
        post(move |Json(page): Json<Value>| async move {
            renders.fetch_add(1, Ordering::SeqCst);
            Json(json!({ "head": ["<title>SSR</title>"], "body": page["component"] }))
        }),
    ))
    .await
}

fn page() -> Page {
    Page {
        component: "Home".to_owned(),
        url: "/home".to_owned(),
        ..Page::default()
    }
}

fn request(path: &str) -> Request {
    Request::new(Method::GET, &path.parse().unwrap(), http::HeaderMap::new())
}

type Failures = Arc<Mutex<Vec<(SsrFailure, String)>>>;

/// A gateway that records its failures, with the page component.
fn recording(url: &str) -> (HttpGateway, Failures) {
    let failures = Failures::default();
    let gateway = HttpGateway::new().url(url).on_failure({
        let failures = failures.clone();
        move |failure, page| failures.lock().unwrap().push((failure.clone(), page.component.clone()))
    });

    (gateway, failures)
}

#[tokio::test]
async fn renders_through_the_ssr_server() {
    let url = renderer(Arc::default()).await;

    let rendered = HttpGateway::new().url(url).dispatch(&page(), &request("/home")).await;

    assert_eq!(
        rendered,
        Some(Rendered {
            head: "<title>SSR</title>".to_owned(),
            body: "Home".to_owned(),
        })
    );
}

#[tokio::test]
async fn render_failures_are_reported() {
    let url = serve(Router::new().route(
        "/render",
        post(|| async {
            let error = json!({
                "error": "window is not defined",
                "type": "browser-api",
                "hint": "Move it into a lifecycle hook",
                "browserApi": "window",
                "stack": "ReferenceError: window is not defined\n    at Home",
                "sourceLocation": "resources/js/pages/Home.vue:12:5",
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error))
        }),
    ))
    .await;
    let (gateway, failures) = recording(&url);

    assert_eq!(gateway.dispatch(&page(), &request("/home")).await, None);

    let failures = failures.lock().unwrap();
    let [(failure, component)] = failures.as_slice() else {
        panic!("expected one failure, got {failures:?}");
    };
    assert_eq!(component, "Home");
    assert_eq!(
        *failure,
        SsrFailure {
            error: "window is not defined".to_owned(),
            kind: SsrErrorType::BrowserApi,
            hint: Some("Move it into a lifecycle hook".to_owned()),
            browser_api: Some("window".to_owned()),
            stack: Some("ReferenceError: window is not defined\n    at Home".to_owned()),
            source_location: Some("resources/js/pages/Home.vue:12:5".to_owned()),
        }
    );
}

#[tokio::test]
async fn unreachable_servers_are_reported_as_connection_failures() {
    let (gateway, failures) = recording(&closed_url().await);

    assert_eq!(gateway.dispatch(&page(), &request("/home")).await, None);

    let failures = failures.lock().unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].0.kind, SsrErrorType::Connection);
    assert!(!failures[0].0.error.is_empty());
}

#[tokio::test]
async fn ssr_can_be_disabled_per_request() {
    let renders = Arc::new(AtomicUsize::new(0));
    let url = renderer(renders.clone()).await;
    let gateway = HttpGateway::new()
        .url(url)
        .disable_when(|request| request.path().starts_with("/admin"));

    assert_eq!(gateway.dispatch(&page(), &request("/admin/users")).await, None);
    assert_eq!(renders.load(Ordering::SeqCst), 0);

    assert!(gateway.dispatch(&page(), &request("/home")).await.is_some());
    assert_eq!(renders.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn disabling_ssr_wins_over_disable_when() {
    let renders = Arc::new(AtomicUsize::new(0));
    let url = renderer(renders.clone()).await;
    let gateway = HttpGateway::new().url(url).enabled(false).disable_when(|_| false);

    assert_eq!(gateway.dispatch(&page(), &request("/home")).await, None);
    assert_eq!(renders.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn requests_and_health_checks_use_the_custom_client() {
    fn authorized(headers: &HeaderMap) -> bool {
        headers.get("x-ssr-token").is_some_and(|token| token == "secret")
    }

    let url = serve(
        Router::new()
            .route(
                "/render",
                post(|headers: HeaderMap| async move {
                    if !authorized(&headers) {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    Json(json!({ "head": [], "body": "authorized" })).into_response()
                }),
            )
            .route(
                "/health",
                get(|headers: HeaderMap| async move {
                    if authorized(&headers) {
                        StatusCode::OK
                    } else {
                        StatusCode::UNAUTHORIZED
                    }
                }),
            ),
    )
    .await;

    let mut headers = http::HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-ssr-token"),
        HeaderValue::from_static("secret"),
    );
    headers.insert(header::USER_AGENT, HeaderValue::from_static("inertia-tests"));
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let plain = HttpGateway::new().url(&url);
    assert!(!plain.is_healthy().await);
    assert_eq!(plain.dispatch(&page(), &request("/home")).await, None);

    let gateway = HttpGateway::new().url(&url).client(client);
    assert!(gateway.is_healthy().await);
    assert_eq!(
        gateway
            .dispatch(&page(), &request("/home"))
            .await
            .map(|rendered| rendered.body),
        Some("authorized".to_owned())
    );
}

#[test]
fn gateways_with_callbacks_are_debug_and_clone() {
    let gateway = HttpGateway::new().disable_when(|_| true).on_failure(|_, _| {});

    let debug = format!("{:?}", gateway.clone());

    assert!(debug.contains("disable_when: true"), "{debug}");
    assert!(debug.contains("on_failure: true"), "{debug}");
}
