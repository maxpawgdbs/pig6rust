use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use pig6bot::{
    certificates::Certificates,
    config::{JsonStore, Settings},
    economy::Economy,
    web::{self, WebState},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;
fn state(dir: &std::path::Path) -> WebState {
    WebState {
        settings: Settings {
            data_dir: dir.into(),
            web_dir: dir.into(),
            owner: 1,
            main_channel: -1,
            personal_channel: -2,
            website: "https://example.com".into(),
            bind: "127.0.0.1:0".into(),
            telegram_api: "http://127.0.0.1:1".into(),
            gemini_model: "unused".into(),
        },
        config: JsonStore::new(dir.join("config.json"), pig6bot::config::defaults()),
        economy: Arc::new(Economy::memory().unwrap()),
        certificates: Certificates::new(dir),
    }
}
#[tokio::test]
async fn api_auth_validation_and_market_contract() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(dir.path());
    st.economy.generate(100).unwrap();
    let key = st.economy.create_key(7, "api").unwrap();
    let app = web::router(st);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/balance")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/balance")
                .header("authorization", format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(value, json!({"balance":100}));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/market")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let value: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(value["availableCodes"], 100);
    assert_eq!(value["price"], 100);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/top?limit=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
#[tokio::test]
async fn policy_is_live_escaped_and_uncached() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("security-policy.html"),
        "<p>{{ policy_text }}</p><a href=\"{{ canonical_url }}\">link</a>",
    )
    .unwrap();
    std::fs::write(dir.path().join("security-policy.txt"), "first").unwrap();
    let app = web::router(state(dir.path()));
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/security-policy")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()["cache-control"], "no-cache");
    std::fs::write(
        dir.path().join("security-policy.txt"),
        "<script>new</script>",
    )
    .unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/security-policy")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let text = text.replace("&#x2f;", "/");
    assert!(text.contains("&lt;script&gt;new&lt;/script&gt;"), "{text}");
    assert!(!text.contains("<script>"));
    assert!(text.contains("https://example.com/security-policy"));
}
#[tokio::test]
async fn shadow_certificate_does_not_disclose_author_on_public_route() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(dir.path());
    std::fs::write(
        dir.path().join("shadow.html"),
        "{{ message }} {{ signature }}",
    )
    .unwrap();
    std::fs::write(dir.path().join("check.html"), "{{ author_name }}").unwrap();
    st.certificates.generate(7).unwrap();
    let cert = st
        .certificates
        .create(&json!({"id":7,"first_name":"secret"}), "hello", true)
        .unwrap();
    let app = web::router(st);
    let sig = cert["signature"].as_str().unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/check?signature={sig}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/shadow?signature={sig}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(!text.contains("secret"));
}
#[tokio::test]
async fn telegram_errors_do_not_include_bot_token() {
    use axum::routing::post;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/botfake-secret/getMe",
        post(|| async { axum::Json(json!({"ok":false,"description":"Denied"})) }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let tg = pig6bot::telegram::Telegram::new("fake-secret".into(), &format!("http://{address}"))
        .unwrap();
    let err = tg.call("getMe", json!({})).await.unwrap_err().to_string();
    assert!(err.contains("Denied"));
    assert!(!err.contains("fake-secret"));
    server.abort();
}
