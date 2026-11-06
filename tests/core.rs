use pig6bot::{
    certificates::{Certificates, SUFFIX, signed_role},
    config::{self, JsonStore},
    economy::Economy,
    moderation::{self, Flood},
    shadow,
};
use serde_json::{Value, json};
use std::sync::Arc;
fn time(s: &str) -> shadow::Time {
    shadow::parse(s).unwrap()
}
fn channel() -> Value {
    json!({"channel_id":-10,"owner":{"id":7,"name":"Owner","username":"owner"},"name":"n","uuid":"u","enabled":true})
}
#[test]
fn calendar_month_clamps_and_uses_moscow() {
    assert_eq!(
        shadow::add_month(time("2024-01-31T13:00:00+03:00")),
        time("2024-02-29T13:00:00+03:00")
    );
    assert_eq!(
        shadow::add_month(time("2025-12-31T13:00:00+03:00")),
        time("2026-01-31T13:00:00+03:00")
    );
    assert_eq!(time("2026-11-06T21:10:00Z"), time("2026-11-07T00:10:00"));
    assert_eq!(
        shadow::next_reset(time("2026-11-07T23:59:59+03:00")),
        time("2026-11-08T00:00:00+03:00")
    );
}
#[test]
fn trial_is_once_and_expiry_falls_back_to_free() {
    let mut d = shadow::defaults();
    let t = time("2026-01-31T12:00:00+03:00");
    assert_eq!(shadow::effective_plan(&mut d, 7, t).unwrap(), "Plus");
    let original = d["subscriptions"]["7"].clone();
    assert_eq!(
        shadow::effective_plan(&mut d, 7, time("2026-03-01T00:00:00+03:00")).unwrap(),
        "Free"
    );
    assert_eq!(d["subscriptions"]["7"], original);
}
#[test]
fn purchases_are_idempotent_extend_same_plan_and_switch_immediately() {
    let mut d = shadow::defaults();
    let t = time("2026-01-15T12:00:00+03:00");
    shadow::ensure_subscription(&mut d, 7, t);
    let p = shadow::grant(&mut d, 7, "Plus", "receipt", t).unwrap();
    assert_eq!(p["expires_at"], "2026-03-15T12:00:00+03:00");
    assert_eq!(shadow::grant(&mut d, 7, "Plus", "receipt", t).unwrap(), p);
    let pro = shadow::grant(&mut d, 7, "Pro x5", "other", t).unwrap();
    assert_eq!(pro["expires_at"], "2026-02-15T12:00:00+03:00");
}
#[test]
fn quotas_are_shared_by_owner_and_albums_cost_one() {
    let mut d = shadow::defaults();
    let t = time("2026-01-15T12:00:00+03:00");
    d["subscriptions"]["7"] = json!({"plan":"Free","expires_at":null});
    let c = channel();
    assert_eq!(
        shadow::reserve(&mut d, &c, -1, 1, t, Some("album")).unwrap(),
        "accepted"
    );
    assert_eq!(
        shadow::reserve(&mut d, &c, -1, 2, t, Some("album")).unwrap(),
        "accepted"
    );
    assert_eq!(
        shadow::reserve(&mut d, &c, -1, 1, t, None).unwrap(),
        "existing"
    );
    let mut c2 = c.clone();
    c2["channel_id"] = json!(-11);
    assert_eq!(
        shadow::reserve(&mut d, &c2, -1, 3, t, None).unwrap(),
        "limit"
    );
    assert_eq!(d["usage"]["7"]["posts"], 1);
    assert_eq!(
        shadow::reserve(&mut d, &c2, -1, 4, t + chrono::Duration::days(1), None).unwrap(),
        "accepted"
    );
}
#[test]
fn refund_never_decrements_next_day() {
    let mut d = shadow::defaults();
    let t = time("2026-01-15T12:00:00+03:00");
    shadow::reserve(&mut d, &channel(), -1, 1, t, None).unwrap();
    shadow::refund(&mut d, "-1:1", t).unwrap();
    assert_eq!(d["usage"]["7"]["posts"], 0);
    shadow::reserve(&mut d, &channel(), -1, 2, t, None).unwrap();
    let tomorrow = t + chrono::Duration::days(1);
    shadow::reserve(&mut d, &channel(), -1, 3, tomorrow, None).unwrap();
    shadow::refund(&mut d, "-1:2", tomorrow).unwrap();
    assert_eq!(d["usage"]["7"]["posts"], 1);
}
#[test]
fn revealed_author_stays_available_after_plan_expiry() {
    let mut d = shadow::defaults();
    let t = time("2026-01-15T12:00:00+03:00");
    d["subscriptions"]["8"] = json!({"plan":"Pro x5","expires_at":"2026-01-16T00:00:00+03:00"});
    shadow::reserve(&mut d, &channel(), -1, 1, t, None).unwrap();
    shadow::reserve(&mut d, &channel(), -1, 2, t, None).unwrap();
    assert_eq!(
        shadow::reveal(&mut d, 8, "unknown", t).unwrap().0,
        "unknown"
    );
    assert_eq!(shadow::reveal(&mut d, 8, "-1:1", t).unwrap().0, "revealed");
    assert_eq!(shadow::reveal(&mut d, 8, "-1:2", t).unwrap().0, "limit");
    assert_eq!(
        shadow::reveal(&mut d, 8, "-1:1", t + chrono::Duration::days(2))
            .unwrap()
            .0,
        "cached"
    );
}
#[test]
fn failed_disclosure_can_be_refunded() {
    let mut d = shadow::defaults();
    let t = time("2026-01-15T12:00:00+03:00");
    d["subscriptions"]["8"] = json!({"plan":"Ultra","expires_at":null});
    shadow::reserve(&mut d, &channel(), -1, 1, t, None).unwrap();
    shadow::reveal(&mut d, 8, "-1:1", t).unwrap();
    shadow::refund_view(&mut d, 8, "-1:1", t);
    assert_eq!(d["usage"]["8"]["views"], 0);
    assert!(
        d["subscriptions"]["8"]["revealed_posts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn malformed_quota_is_rejected() {
    let mut d = shadow::defaults();
    for bad in [json!(-1), json!(1.5), json!("5"), json!(true)] {
        d["plans"]["Plus"]["posts_per_day"] = bad;
        assert!(shadow::validate(&d).is_err());
    }
}
#[test]
fn store_rejects_admin_and_other_users() {
    let mut msg = json!({"chat":{"is_direct_messages":true},"direct_messages_topic":{"user":{"id":7}},"from":{"id":8,"is_bot":false}});
    assert!(shadow::check_store_user(&msg, &json!("Plus")).is_err());
    msg["from"]["id"] = json!(7);
    assert_eq!(shadow::check_store_user(&msg, &json!("Plus")).unwrap(), 7);
}
#[test]
fn json_writes_are_serialized_and_preserve_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.json");
    std::fs::write(&path, r#"{"n":0,"custom":true}"#).unwrap();
    let store = Arc::new(JsonStore::new(path, json!({"n":0})));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || {
                for _ in 0..20 {
                    store
                        .transaction(|d| {
                            d["n"] = json!(d["n"].as_i64().unwrap() + 1);
                            Ok(())
                        })
                        .unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let d = store.read().unwrap();
    assert_eq!(d["n"], 80);
    assert_eq!(d["custom"], true);
}
#[test]
fn malformed_json_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.json");
    std::fs::write(&path, b"{invalid").unwrap();
    let store = JsonStore::new(&path, json!({}));
    assert!(store.transaction(|_| Ok(())).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"{invalid");
}
#[test]
fn errors_rollback_json_transactions() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonStore::new(dir.path().join("data.json"), json!({"n":1}));
    store.transaction(|_| Ok(())).unwrap();
    let result: anyhow::Result<()> = store.transaction(|d| {
        d["n"] = json!(2);
        anyhow::bail!("abort")
    });
    assert!(result.is_err());
    assert_eq!(store.read().unwrap()["n"], 1);
}
#[test]
fn transfers_are_atomic_and_reject_invalid_amounts() {
    let e = Economy::memory().unwrap();
    e.add_user(1, Some("alice")).unwrap();
    e.add_user(2, Some("bob")).unwrap();
    assert_eq!(e.transfer(1, 2, 75, "test").unwrap(), 25);
    assert_eq!(e.balance(2).unwrap(), 175);
    assert!(e.transfer(1, 2, 26, "test").is_err());
    assert!(e.transfer(1, 2, -50, "test").is_err());
    assert!(e.transfer(1, 1, 1, "test").is_err());
    assert_eq!(e.balance(1).unwrap(), 25);
    assert_eq!(e.balance(2).unwrap(), 175);
}
#[test]
fn concurrent_overspending_cannot_occur() {
    let e = Arc::new(Economy::memory().unwrap());
    e.add_user(1, None).unwrap();
    e.add_user(2, None).unwrap();
    let threads: Vec<_> = (0..10)
        .map(|_| {
            let e = e.clone();
            std::thread::spawn(move || e.transfer(1, 2, 25, "parallel").is_ok())
        })
        .collect();
    let successes = threads
        .into_iter()
        .map(|t| t.join().unwrap() as usize)
        .sum::<usize>();
    assert_eq!(successes, 4);
    assert_eq!(e.balance(1).unwrap(), 0);
    assert_eq!(e.balance(2).unwrap(), 200);
}
#[test]
fn market_purchase_failure_rolls_back_everything() {
    let e = Economy::memory().unwrap();
    e.generate(3).unwrap();
    assert!(e.trade(1, 2, true, &config::defaults()).is_err());
    assert_eq!(e.system_count().unwrap(), 3);
    assert!(e.codes(1).unwrap().is_empty());
    assert_eq!(e.balance(1).unwrap(), 100);
    assert!(e.operations().unwrap().as_array().unwrap().is_empty());
}
#[test]
fn buy_sell_and_code_redemption_preserve_inventory() {
    let e = Economy::memory().unwrap();
    e.generate(100).unwrap();
    e.transfer(0, 1, 1000, "fund").unwrap();
    let c = config::defaults();
    e.trade(1, 2, true, &c).unwrap();
    assert_eq!(e.system_count().unwrap(), 98);
    assert_eq!(e.codes(1).unwrap().len(), 2);
    let code = e.codes(1).unwrap()[0].clone();
    assert!(e.redeem(&format!("post {code}")).unwrap());
    assert!(!e.redeem(&code).unwrap());
    e.trade(1, 1, false, &c).unwrap();
    assert_eq!(e.system_count().unwrap(), 99);
    assert!(e.codes(1).unwrap().is_empty());
    e.replenish(100).unwrap();
    assert_eq!(e.system_count().unwrap(), 100);
}
#[test]
fn api_keys_are_owner_scoped_and_revocable() {
    let e = Economy::memory().unwrap();
    let key = e.create_key(7, "test").unwrap();
    assert_eq!(e.authenticate(&key).unwrap(), Some(7));
    assert_eq!(e.authenticate("bad").unwrap(), None);
    let key_id = e.keys(7).unwrap()[0]["id"].as_i64().unwrap();
    assert!(!e.revoke(8, key_id).unwrap());
    assert!(e.revoke(7, key_id).unwrap());
    assert_eq!(e.authenticate(&key).unwrap(), None);
}
#[test]
fn certificate_pem_roundtrip_rejects_tampering() {
    let dir = tempfile::tempdir().unwrap();
    let c = Certificates::new(dir.path());
    c.generate(7).unwrap();
    let text = "Привет 🙂";
    let sig = c.sign(7, text).unwrap();
    assert!(c.verify(7, text, &sig));
    assert!(!c.verify(7, "changed", &sig));
    assert!(!c.verify(8, text, &sig));
    c.generate(7).unwrap();
    assert!(c.verify(7, text, &sig));
    assert!(c.read("../config").is_err());
}
#[test]
fn certificate_role_requires_a_valid_signature() {
    let dir = tempfile::tempdir().unwrap();
    let c = Certificates::new(dir.path());
    c.generate(7).unwrap();
    let sig = c.sign(7, "Hi 🙂").unwrap();
    let config = json!({"signed_users":{"7":{"role":"root"}}});
    let msg = json!({"text":format!("Hi 🙂{SUFFIX}"),"entities":[{"type":"text_link","url":format!("https://example.com/check?signature={sig}&user_id=7")} ]});
    assert_eq!(signed_role(&c, &config, &msg), "root");
    let mut forged = msg;
    forged["text"] = json!(format!("Changed{SUFFIX}"));
    assert_eq!(signed_role(&c, &config, &forged), "");
}
#[test]
fn filtering_normalizes_duplicates_and_case() {
    let config = json!({"banned":["bad"],"ban_messages":"manual"});
    let dict = json!({"b":"b","a":"a","d":"d","4":"a"});
    assert!(moderation::banned("B444D", &config, &dict));
    assert!(!moderation::banned("good", &config, &dict));
}
#[test]
fn flood_threshold_and_cleanup_are_per_chat() {
    let mut flood = Flood::default();
    for i in 1..10 {
        assert!(flood.record(-1, i).is_empty());
    }
    assert!(flood.record(-2, 99).is_empty());
    assert_eq!(flood.record(-1, 10), (1..=10).collect::<Vec<_>>());
    assert!(flood.record(-1, 11).is_empty());
}
#[test]
fn telegram_links_and_invisible_identifiers() {
    assert_eq!(
        shadow::post_link("https://t.me/c/123/42?single").unwrap(),
        Some(("c/123".into(), 42))
    );
    assert_eq!(
        shadow::post_link("https://t.me/channel/42").unwrap(),
        Some(("channel".into(), 42))
    );
    assert!(shadow::post_link("https://t.me/channel").unwrap().is_none());
    assert!(shadow::invisible(&config::invisible_id()));
}
#[test]
fn signatures_match_python_cryptography_with_unicode_and_whitespace() {
    // Public test key derived from bytes 0..31; never use this fixture in production.
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/python-ed25519.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let certs = Certificates::new(dir.path());
    std::fs::create_dir_all(dir.path().join("keys/private")).unwrap();
    std::fs::create_dir_all(dir.path().join("keys/public")).unwrap();
    std::fs::write(
        dir.path().join("keys/private/7.private.pem"),
        fixture["private_pem"].as_str().unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("keys/public/7.public.pem"),
        fixture["public_pem"].as_str().unwrap(),
    )
    .unwrap();
    let text = fixture["text"].as_str().unwrap();
    let signature = fixture["signature"].as_str().unwrap();
    assert!(certs.verify(7, text, signature));
    assert_eq!(certs.sign(7, text).unwrap(), signature);
    let config = json!({"signed_users":{"7":{"role":"user"}}});
    let msg = json!({"text":format!("{text}{SUFFIX}"),"entities":[{"type":"text_link","url":format!("https://example.com/check?signature={signature}&user_id=7")}]});
    assert_eq!(signed_role(&certs, &config, &msg), "user");
}
