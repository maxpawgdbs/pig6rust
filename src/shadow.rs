//! Shadow state transitions are independent of Telegram and use Moscow calendar days.
use crate::config::{JsonStore, id, s};
use anyhow::{Result, bail, ensure};
use chrono::{DateTime, FixedOffset, Months, NaiveDateTime, TimeZone, Utc};
use serde_json::{Value, json};
pub type Time = DateTime<FixedOffset>;
pub fn now() -> Time {
    Utc::now().with_timezone(&FixedOffset::east_opt(10800).unwrap())
}
pub fn parse(value: &str) -> Result<Time> {
    if let Ok(v) = DateTime::parse_from_rfc3339(value) {
        return Ok(v.with_timezone(&FixedOffset::east_opt(10800).unwrap()));
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")?;
    Ok(FixedOffset::east_opt(10800)
        .unwrap()
        .from_local_datetime(&naive)
        .single()
        .unwrap())
}
pub fn add_month(t: Time) -> Time {
    t.checked_add_months(Months::new(1)).unwrap()
}
pub fn next_reset(t: Time) -> Time {
    let day = t.date_naive().succ_opt().unwrap();
    t.timezone()
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
}
pub fn reset_text(t: Time) -> String {
    let mins = (next_reset(t) - t).num_minutes();
    format!("{} ч {} мин", mins / 60, mins % 60)
}
pub fn defaults() -> Value {
    json!({"plans":{"Free":{"posts_per_day":1,"views_per_day":0},"Plus":{"posts_per_day":5,"views_per_day":0},"Pro x5":{"posts_per_day":25,"views_per_day":1},"Pro x20":{"posts_per_day":100,"views_per_day":5},"Ultra":{"posts_per_day":1000,"views_per_day":100}},"stores":{"pig6storeplus":"Plus","pig6store_pro_x5":"Pro x5","pig6store_pro_x20":"Pro x20"},"subscriptions":{},"usage":{},"channels":{},"posts":{},"purchases":{}})
}
pub fn validate(data: &Value) -> Result<()> {
    for key in [
        "plans",
        "stores",
        "subscriptions",
        "usage",
        "channels",
        "posts",
        "purchases",
    ] {
        ensure!(data[key].is_object(), "Shadow {key} must be an object");
    }
    for (_, plan) in data["plans"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("plans must be an object"))?
    {
        for key in ["posts_per_day", "views_per_day"] {
            ensure!(plan[key].as_u64().is_some(), "Invalid Shadow quota: {key}");
        }
    }
    Ok(())
}
pub fn ensure_subscription(data: &mut Value, user: i64, t: Time) -> &mut Value {
    let key = user.to_string();
    if data["subscriptions"].get(&key).is_none() {
        let expiry = add_month(t).to_rfc3339();
        data["subscriptions"][&key] = json!({"plan":"Plus","expires_at":expiry,"trial_started_at":t.to_rfc3339(),"trial_expires_at":expiry});
    }
    &mut data["subscriptions"][&key]
}
pub fn effective_plan(data: &mut Value, user: i64, t: Time) -> Result<String> {
    let sub = ensure_subscription(data, user, t);
    let plan = s(&sub["plan"]).to_string();
    let expiry = sub["expires_at"].as_str().map(str::to_owned);
    if data["plans"].get(&plan).is_none() {
        return Ok("Free".into());
    }
    if let Some(expiry) = expiry {
        if parse(&expiry)? <= t {
            return Ok("Free".into());
        }
    }
    Ok(plan)
}
pub fn daily_usage(data: &mut Value, user: i64, t: Time) -> &mut Value {
    let key = user.to_string();
    let date = t.format("%Y-%m-%d").to_string();
    if s(&data["usage"][&key]["date"]) != date {
        data["usage"][&key] = json!({"date":date,"posts":0,"views":0});
    }
    &mut data["usage"][&key]
}
pub fn grant(data: &mut Value, user: i64, plan: &str, receipt: &str, t: Time) -> Result<Value> {
    validate(data)?;
    if let Some(p) = data["purchases"].get(receipt) {
        return Ok(p.clone());
    }
    ensure!(
        plan != "Free" && data["plans"].get(plan).is_some(),
        "Invalid store subscription"
    );
    let current = effective_plan(data, user, t)?;
    let sub = ensure_subscription(data, user, t);
    let base = if current == plan {
        sub["expires_at"]
            .as_str()
            .map(parse)
            .transpose()?
            .unwrap_or(t)
            .max(t)
    } else {
        t
    };
    sub["plan"] = json!(plan);
    sub["expires_at"] = json!(add_month(base).to_rfc3339());
    let result = json!({"user_id":user,"plan":plan,"expires_at":sub["expires_at"],"created_at":t.to_rfc3339()});
    data["purchases"][receipt] = result.clone();
    Ok(result)
}
pub fn reserve(
    data: &mut Value,
    channel: &Value,
    chat: i64,
    message: i64,
    t: Time,
    album: Option<&str>,
) -> Result<&'static str> {
    validate(data)?;
    let key = format!("{chat}:{message}");
    if data["posts"].get(&key).is_some() {
        return Ok("existing");
    }
    let user = id(&channel["owner"]["id"]);
    let plan = effective_plan(data, user, t)?;
    let is_album = album.is_some_and(|a| {
        data["posts"].as_object().unwrap().values().any(|p| {
            s(&p["media_group_id"]) == a
                && id(&p["chat_id"]) == chat
                && p["channel_id"] == channel["channel_id"]
        })
    });
    let limit = id(&data["plans"][&plan]["posts_per_day"]);
    let usage = daily_usage(data, user, t);
    if !is_album && id(&usage["posts"]) >= limit {
        return Ok("limit");
    }
    if !is_album {
        usage["posts"] = json!(id(&usage["posts"]) + 1)
    }
    data["posts"][&key] = json!({"chat_id":chat,"message_id":message,"channel_id":channel["channel_id"],"owner":channel["owner"],"created_at":t.to_rfc3339(),"media_group_id":album,"signature":format!("{}{}",s(&channel["name"]),s(&channel["uuid"])),"charged":!is_album});
    Ok("accepted")
}
pub fn refund(data: &mut Value, key: &str, t: Time) -> Result<()> {
    let Some(post) = data["posts"].as_object_mut().unwrap().remove(key) else {
        return Ok(());
    };
    if post["charged"] == true && parse(s(&post["created_at"]))?.date_naive() == t.date_naive() {
        let usage = daily_usage(data, id(&post["owner"]["id"]), t);
        usage["posts"] = json!((id(&usage["posts"]) - 1).max(0));
    }
    Ok(())
}
pub fn reveal(data: &mut Value, user: i64, key: &str, t: Time) -> Result<(&'static str, Value)> {
    validate(data)?;
    let Some(post) = data["posts"].get(key).cloned() else {
        return Ok(("unknown", Value::Null));
    };
    let sub = ensure_subscription(data, user, t);
    if sub["revealed_posts"].is_null() {
        sub["revealed_posts"] = json!([])
    }
    if sub["revealed_posts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == key)
    {
        return Ok(("cached", post["owner"].clone()));
    }
    let plan = effective_plan(data, user, t)?;
    let limit = id(&data["plans"][&plan]["views_per_day"]);
    let usage = daily_usage(data, user, t);
    if id(&usage["views"]) >= limit {
        return Ok(("limit", Value::Null));
    }
    usage["views"] = json!(id(&usage["views"]) + 1);
    data["subscriptions"][user.to_string()]["revealed_posts"]
        .as_array_mut()
        .unwrap()
        .push(json!(key));
    Ok(("revealed", post["owner"].clone()))
}
pub fn refund_view(data: &mut Value, user: i64, key: &str, t: Time) {
    if let Some(posts) = data["subscriptions"][user.to_string()]["revealed_posts"].as_array_mut() {
        if let Some(i) = posts.iter().position(|p| p == key) {
            posts.remove(i);
            let usage = daily_usage(data, user, t);
            usage["views"] = json!((id(&usage["views"]) - 1).max(0));
        }
    }
}
pub fn subscription_view(store: &JsonStore, user: i64) -> Result<String> {
    store.transaction(|d| {
        validate(d)?;
        let t = now();
        let plan = effective_plan(d, user, t)?;
        let sub = d["subscriptions"][user.to_string()].clone();
        let limits = d["plans"][&plan].clone();
        let usage = daily_usage(d, user, t).clone();
        fn bar(used: i64, max: i64) -> String {
            let n = if max > 0 {
                ((max - used).max(0) * 20 / max).min(20)
            } else {
                0
            } as usize;
            format!("{}{}", "▓".repeat(n), "░".repeat(20 - n))
        }
        let mut text = format!(
            "Подписка: {plan}\nСрок действия: {}\n\n",
            sub["expires_at"].as_str().unwrap_or("бессрочно")
        );
        if sub["trial_expires_at"]
            .as_str()
            .map(parse)
            .transpose()?
            .is_some_and(|e| e > t)
        {
            text.push_str("Бесплатный период Plus\n\n");
        }
        for (title, used, max) in [
            ("Публикации", "posts", "posts_per_day"),
            ("Просмотры авторства", "views", "views_per_day"),
        ] {
            let max = id(&limits[max]);
            let used = id(&usage[used]);
            text.push_str(&format!(
                "{title}: {} из {max}\n{}\n\n",
                (max - used).max(0),
                bar(used, max)
            ));
        }
        text.push_str(&format!(
            "Сброс лимитов через {} (00:00 по Москве).",
            reset_text(t)
        ));
        Ok(text)
    })
}
pub fn invisible(title: &str) -> bool {
    !title.is_empty()
        && title
            .chars()
            .all(|c| ('\u{E0100}'..='\u{E01EF}').contains(&c))
}
pub fn channel_name(data: &Value, previous: Option<&Value>) -> String {
    if let Some(p) = previous {
        if invisible(s(&p["name"])) {
            return s(&p["name"]).into();
        }
    }
    loop {
        let name = crate::config::invisible_id();
        if !data["channels"]
            .as_object()
            .unwrap()
            .values()
            .any(|c| c["name"] == name)
        {
            return name;
        }
    }
}
pub fn owner_info(user: &Value) -> Value {
    let name = [s(&user["first_name"]), s(&user["last_name"])]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    json!({"id":id(&user["id"]),"name":name,"username":user["username"]})
}
pub fn post_link(text: &str) -> Result<Option<(String, i64)>> {
    let re = regex::Regex::new(
        r"(?:https?://)?t\.me/(?P<chat>c/\d+|[A-Za-z][A-Za-z0-9_]*?)/(?P<message>\d+)(?:\?|\s|$)",
    )?;
    let Some(c) = re.captures(text) else {
        return Ok(None);
    };
    Ok(Some((c["chat"].to_string(), c["message"].parse()?)))
}
pub fn check_store_user(msg: &Value, plan: &Value) -> Result<i64> {
    if plan.is_null() || !msg["chat"]["is_direct_messages"].as_bool().unwrap_or(false) {
        bail!("Not a store message")
    }
    let user = id(&msg["direct_messages_topic"]["user"]["id"]);
    ensure!(
        user != 0 && user == id(&msg["from"]["id"]) && msg["from"]["is_bot"] != true,
        "Not a customer message"
    );
    Ok(user)
}
