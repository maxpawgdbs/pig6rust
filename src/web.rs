use crate::{
    certificates::Certificates,
    config::{JsonStore, Settings, id, s},
    economy::Economy,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
#[derive(Clone)]
pub struct WebState {
    pub settings: Settings,
    pub config: JsonStore,
    pub economy: Arc<Economy>,
    pub certificates: Certificates,
}
type ApiResult = Result<Json<Value>, ApiError>;
pub struct ApiError(pub StatusCode, pub String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"detail":self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::warn!(error=%e,"API request failed");
        Self(StatusCode::BAD_REQUEST, e.to_string())
    }
}
pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/market", get(market))
        .route("/api/history", get(history))
        .route("/api/top", get(top))
        .route("/api/operations", get(operations))
        .route("/api/balance", get(balance))
        .route("/api/buy", post(buy))
        .route("/api/sell", post(sell))
        .route("/api/pay", post(pay))
        .route("/api/bonus", post(bonus))
        .route("/check", get(check))
        .route("/shadow", get(shadow_cert))
        .route("/security-policy", get(policy))
        .route("/", get(index))
        .route("/{file}", get(static_file))
        .fallback(not_found)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
pub async fn serve(state: WebState) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(&state.settings.bind).await?;
    tracing::info!(address=%state.settings.bind,"HTTP server listening");
    axum::serve(listener, router(state)).await?;
    Ok(())
}
fn authenticate(state: &WebState, headers: &HeaderMap) -> Result<i64, ApiError> {
    let key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Invalid API key".into()))?;
    state
        .economy
        .authenticate(key)?
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Invalid API key".into()))
}
async fn market(State(st): State<WebState>) -> ApiResult {
    Ok(Json(st.economy.market(&st.config.read()?)?))
}
async fn operations(State(st): State<WebState>) -> ApiResult {
    Ok(Json(st.economy.operations()?))
}
#[derive(Deserialize)]
struct History {
    #[serde(default = "default_range")]
    range: String,
}
fn default_range() -> String {
    "24H".into()
}
async fn history(State(st): State<WebState>, Query(q): Query<History>) -> ApiResult {
    let hours = match q.range.as_str() {
        "1H" => 1,
        "6H" => 6,
        "24H" => 24,
        "7D" => 168,
        "30D" => 720,
        _ => {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Invalid range".into(),
            ));
        }
    };
    Ok(Json(st.economy.history(hours)?))
}
#[derive(Deserialize)]
struct Top {
    #[serde(default = "default_limit")]
    limit: i64,
}
fn default_limit() -> i64 {
    10
}
async fn top(State(st): State<WebState>, Query(q): Query<Top>) -> ApiResult {
    if !(1..=1000).contains(&q.limit) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid limit".into(),
        ));
    }
    Ok(Json(st.economy.top(q.limit)?))
}
async fn balance(State(st): State<WebState>, headers: HeaderMap) -> ApiResult {
    let user = authenticate(&st, &headers)?;
    Ok(Json(json!({"balance":st.economy.balance(user)?})))
}
#[derive(Deserialize)]
struct Count {
    #[serde(default = "one")]
    count: i64,
}
fn one() -> i64 {
    1
}
async fn buy(State(st): State<WebState>, headers: HeaderMap, Query(q): Query<Count>) -> ApiResult {
    let user = authenticate(&st, &headers)?;
    Ok(Json(st.economy.trade(
        user,
        q.count,
        true,
        &st.config.read()?,
    )?))
}
async fn sell(State(st): State<WebState>, headers: HeaderMap, Query(q): Query<Count>) -> ApiResult {
    let user = authenticate(&st, &headers)?;
    Ok(Json(st.economy.trade(
        user,
        q.count,
        false,
        &st.config.read()?,
    )?))
}
#[derive(Deserialize)]
struct Pay {
    username: String,
    amount: i64,
}
async fn pay(State(st): State<WebState>, headers: HeaderMap, Query(q): Query<Pay>) -> ApiResult {
    let user = authenticate(&st, &headers)?;
    let to = st
        .economy
        .user_by_name(&q.username)?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "User not found".into()))?;
    let balance = st.economy.transfer(user, to, q.amount, "user transfer")?;
    Ok(Json(
        json!({"status":"ok","sender_id":user,"receiver_id":to,"username":q.username,"amount":q.amount,"balance":balance}),
    ))
}
async fn bonus(State(st): State<WebState>, headers: HeaderMap) -> ApiResult {
    let user = authenticate(&st, &headers)?;
    Ok(Json(st.economy.bonus(user)?))
}
fn render(st: &WebState, file: &str, context: Value) -> anyhow::Result<String> {
    let source = std::fs::read_to_string(st.settings.web_dir.join(file))?;
    let mut env = minijinja::Environment::new();
    env.add_template(file, &source)?;
    Ok(env.get_template(file)?.render(context)?)
}
async fn policy(State(st): State<WebState>) -> Result<Response, ApiError> {
    let text = std::fs::read_to_string(st.settings.web_dir.join("security-policy.txt"))
        .map_err(anyhow::Error::from)?;
    let html = render(
        &st,
        "security-policy.html",
        json!({"policy_text":text,"canonical_url":st.settings.policy_url()}),
    )?;
    Ok(([("cache-control", "no-cache")], Html(html)).into_response())
}
async fn certificate(
    st: WebState,
    q: HashMap<String, String>,
    shadow: bool,
) -> Result<Html<String>, ApiError> {
    let sig = q
        .get("signature")
        .ok_or_else(|| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Missing signature".into()))?;
    let cert = st
        .certificates
        .read(sig)
        .map_err(|_| ApiError(StatusCode::NOT_FOUND, "Certificate not found".into()))?;
    if cert["shadow"].as_bool().unwrap_or(false) != shadow {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Certificate not found".into(),
        ));
    }
    let user = id(&cert["author"]["id"]);
    let name = [
        s(&cert["author"]["first_name"]),
        s(&cert["author"]["last_name"]),
        s(&cert["author"]["username"]),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    Ok(Html(render(
        &st,
        if shadow { "shadow.html" } else { "check.html" },
        json!({"message":format!("{}...",s(&cert["message"]).chars().take(5).collect::<String>()),"author_name":name,"created_at":cert["created_at"],"cert_id":cert["id"],"signature":cert["signature"],"public_key":st.certificates.public_pem(user).unwrap_or_else(|_|"Public key not found".into())}),
    )?))
}
async fn check(
    State(st): State<WebState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Html<String>, ApiError> {
    certificate(st, q, false).await
}
async fn shadow_cert(
    State(st): State<WebState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Html<String>, ApiError> {
    certificate(st, q, true).await
}
async fn index(State(st): State<WebState>) -> Response {
    asset(&st, "index.html").await
}
async fn static_file(State(st): State<WebState>, Path(file): Path<String>) -> Response {
    let name = match file.as_str() {
        "market" => "market.html",
        "verify" => "verify.html",
        "api-docs" => "api-docs.html",
        "styles.css" | "common.css" | "site.js" | "marketApi.js" | "app.js" => &file,
        _ => return not_found(State(st)).await,
    };
    asset(&st, name).await
}
async fn asset(st: &WebState, name: &str) -> Response {
    match tokio::fs::read(st.settings.web_dir.join(name)).await {
        Ok(bytes) => {
            let content_type = if name.ends_with(".css") {
                "text/css; charset=utf-8"
            } else if name.ends_with(".js") {
                "application/javascript; charset=utf-8"
            } else {
                "text/html; charset=utf-8"
            };
            ([(axum::http::header::CONTENT_TYPE, content_type)], bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}
async fn not_found(State(st): State<WebState>) -> Response {
    let html = tokio::fs::read_to_string(st.settings.web_dir.join("404.html"))
        .await
        .unwrap_or_else(|_| "Not found".into());
    (StatusCode::NOT_FOUND, Html(html)).into_response()
}
