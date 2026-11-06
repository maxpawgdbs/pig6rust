use super::*;
use axum::{Json, Router, extract::State, http::Uri, routing::post};
use std::sync::Mutex;
type Calls = Arc<Mutex<Vec<(String, Value)>>>;
async fn mock(
    State((calls, fail)): State<(Calls, bool)>,
    uri: Uri,
    Json(body): Json<Value>,
) -> Json<Value> {
    let method = uri.path().rsplit('/').next().unwrap().to_string();
    calls.lock().unwrap().push((method.clone(), body));
    if fail && method == "setChatTitle" {
        Json(json!({"ok":false,"description":"No permission"}))
    } else {
        Json(json!({"ok":true,"result":true}))
    }
}
async fn setup(fail: bool) -> (tempfile::TempDir, Bot, Calls, tokio::task::JoinHandle<()>) {
    let dir = tempfile::tempdir().unwrap();
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = Router::new()
        .route("/{*path}", post(mock))
        .with_state((calls.clone(), fail));
    let job = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    let settings = Settings {
        data_dir: dir.path().into(),
        web_dir: std::path::PathBuf::from("web"),
        owner: 1,
        main_channel: -1001,
        personal_channel: -1002,
        website: "https://example.com".into(),
        bind: "127.0.0.1:0".into(),
        telegram_api: format!("http://{address}"),
        gemini_model: "unused".into(),
    };
    let config = settings.config();
    config.transaction(|_| Ok(())).unwrap();
    let shadow = JsonStore::new(dir.path().join("shadow.json"), shadow::defaults());
    shadow.transaction(|d|{d["channels"]["-10"]=json!({"channel_id":-10,"owner":{"id":7,"name":"Author","username":"author"},"name":config::invisible_id(),"uuid":config::invisible_id(),"enabled":true});d["subscriptions"]["7"]=json!({"plan":"Free","expires_at":null});Ok(())}).unwrap();
    let app = App {
        settings: settings.clone(),
        config,
        shadow,
        economy: Arc::new(Economy::memory().unwrap()),
        certificates: Certificates::new(dir.path()),
        dictionary: json!({}),
    };
    let bot = Bot {
        app,
        tg: Telegram::new("test".into(), &settings.telegram_api).unwrap(),
        kind: Kind::Main,
        me: json!({"id":99}),
        flood: Flood::default(),
        jarvis: None,
    };
    (dir, bot, calls, job)
}
fn update(bot: &Bot, message: i64) -> Value {
    let d = bot.app.shadow.read().unwrap();
    let c = &d["channels"]["-10"];
    json!({"channel_post":{"message_id":message,"chat":{"id":-1001,"type":"channel"},"sender_chat":{"id":-10},"author_signature":format!("{}{}",s(&c["name"]),s(&c["uuid"])),"text":"hello"}})
}
#[tokio::test]
async fn shadow_rotation_failure_refunds_quota_and_disables_channel() {
    let (_dir, mut bot, calls, job) = setup(true).await;
    let u = update(&bot, 1);
    bot.handle(&u).await.unwrap();
    let d = bot.app.shadow.read().unwrap();
    assert_eq!(d["usage"]["7"]["posts"], 0);
    assert_eq!(d["channels"]["-10"]["enabled"], false);
    assert!(d["posts"].as_object().unwrap().is_empty());
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|(m, b)| m == "deleteMessage" && b["message_id"] == 1)
    );
    job.abort();
}
#[tokio::test]
async fn edited_shadow_post_has_no_effect() {
    let (_dir, mut bot, calls, job) = setup(false).await;
    let u = update(&bot, 1);
    bot.handle(&u).await.unwrap();
    let before = bot.app.shadow.read().unwrap();
    calls.lock().unwrap().clear();
    bot.handle(&json!({"edited_channel_post":u["channel_post"]}))
        .await
        .unwrap();
    assert_eq!(before, bot.app.shadow.read().unwrap());
    assert!(calls.lock().unwrap().is_empty());
    job.abort();
}
#[tokio::test]
async fn album_old_signature_is_accepted_once_per_album() {
    let (_dir, mut bot, calls, job) = setup(false).await;
    let mut u = update(&bot, 1);
    u["channel_post"]["media_group_id"] = json!("album");
    bot.handle(&u).await.unwrap();
    u["channel_post"]["message_id"] = json!(2);
    bot.handle(&u).await.unwrap();
    let d = bot.app.shadow.read().unwrap();
    assert_eq!(d["usage"]["7"]["posts"], 1);
    assert_eq!(d["posts"].as_object().unwrap().len(), 2);
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == "setChatTitle")
            .count(),
        1
    );
    job.abort();
}
#[tokio::test]
async fn forged_shadow_sender_is_deleted_without_spending_quota() {
    let (_dir, mut bot, calls, job) = setup(false).await;
    let mut u = update(&bot, 1);
    u["channel_post"]["sender_chat"]["id"] = json!(-20);
    bot.handle(&u).await.unwrap();
    assert!(
        bot.app.shadow.read().unwrap()["posts"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|(m, _)| m == "deleteMessage")
    );
    job.abort();
}
#[tokio::test]
async fn unprivileged_user_cannot_approve_download() {
    let (_dir, mut bot, calls, job) = setup(false).await;
    let token = bot
        .save_request(
            "download",
            json!({"user":8,"key":"key","message":{"text":"secret"}}),
        )
        .unwrap();
    let before = bot.app.requests().read().unwrap();
    let q = json!({"callback_query":{"id":"q","from":{"id":8},"message":{"chat":{"id":8,"type":"private"},"message_id":1},"data":format!("approve^{token}")}});
    assert!(bot.handle(&q).await.is_err());
    assert_eq!(before, bot.app.requests().read().unwrap());
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|(m, _)| m == "sendMessage")
    );
    job.abort();
}
#[tokio::test]
async fn download_preserves_original_text_entities() {
    let (_dir, bot, calls, job) = setup(false).await;
    let original = json!({"text":"🙂 link","entities":[{"type":"text_link","offset":3,"length":4,"url":"https://example.com"}]});
    bot.deliver(8, &original).await.unwrap();
    let calls = calls.lock().unwrap();
    let (_, body) = calls.iter().find(|(m, _)| m == "sendMessage").unwrap();
    assert_eq!(body["text"], original["text"]);
    assert_eq!(body["entities"], original["entities"]);
    assert!(body.get("parse_mode").is_none());
    job.abort();
}
#[test]
fn quote_is_a_transparent_webp_sticker() {
    let bytes =
        super::quote::render_quote("Тестовая цитата\nwith another line", "Автор", 7, None).unwrap();
    let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
    assert_eq!(image.width().max(image.height()), 512);
    assert_eq!(image.get_pixel(0, 0).0[3], 0);
    assert!(bytes.len() < 512 * 1024);
}
#[tokio::test]
async fn quote_requires_approval_and_does_not_publish_original_text() {
    let (_dir, bot, calls, job) = setup(false).await;
    let msg = json!({"chat":{"id":-1001,"type":"channel"},"from":{"id":8},"reply_to_message":{"message_id":3,"text":"secret original text","from":{"id":7,"first_name":"Name"}}});
    bot.quote(&msg).await.unwrap();
    let requests = bot.app.requests().read().unwrap();
    assert_eq!(
        requests["pending"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()["kind"],
        "quote"
    );
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|(m, _)| m == "sendPhoto" || m == "sendSticker")
    );
    job.abort();
}
#[test]
fn imports_legacy_download_permissions_once() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    std::fs::write(
        dir.path().join("tmp/allowed_messages.json"),
        r#"{"messages":["-10042"]}"#,
    )
    .unwrap();
    let settings = Settings::from_env(dir.path().into(), std::path::PathBuf::from("web")).unwrap();
    let app = App::new(settings.clone()).unwrap();
    assert_eq!(
        app.requests().read().unwrap()["allowed_downloads"],
        json!(["-10042"])
    );
    std::fs::write(
        dir.path().join("tmp/allowed_messages.json"),
        r#"{"messages":["another"]}"#,
    )
    .unwrap();
    let app = App::new(settings).unwrap();
    assert_eq!(
        app.requests().read().unwrap()["allowed_downloads"],
        json!(["-10042"])
    );
}
