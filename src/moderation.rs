use crate::config::{id, list, s};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};
pub fn text(msg: &Value) -> &str {
    msg["text"]
        .as_str()
        .or(msg["caption"].as_str())
        .or(msg["poll"]["description"].as_str())
        .or(msg["poll"]["question"].as_str())
        .unwrap_or("")
}
pub fn banned(text: &str, config: &Value, dictionary: &Value) -> bool {
    let text = text.to_lowercase();
    let mut normalized = String::new();
    let mut last = String::new();
    for c in text.chars() {
        if let Some(v) = dictionary[c.to_string()].as_str() {
            if normalized.is_empty() || last != v {
                normalized.push_str(v);
                last = v.into();
            }
        }
    }
    list(&config["banned"])
        .iter()
        .filter_map(Value::as_str)
        .filter(|s| !s.is_empty())
        .any(|word| text.contains(word) || normalized.contains(word))
}
pub fn blocked_content(msg: &Value, config: &Value, dictionary: &Value) -> bool {
    let gif = s(&msg["animation"]["file_id"]);
    config["ban_messages"] == "all"
        || (config["ban_messages"] == "manual" && banned(text(msg), config, dictionary))
        || (!gif.is_empty() && list(&config["bad_gifs"]).iter().any(|v| v == gif))
}
pub fn banned_identity(msg: &Value, config: &Value) -> bool {
    list(&config["banned_users"]).iter().any(|v| {
        v == &msg["author_signature"]
            || (!msg["from"].is_null() && id(v) == id(&msg["from"]["id"]) && id(v) != 0)
    })
}
#[derive(Default)]
pub struct Flood {
    chats: HashMap<i64, VecDeque<(Instant, i64)>>,
}
impl Flood {
    pub fn record(&mut self, chat: i64, message: i64) -> Vec<i64> {
        let now = Instant::now();
        let q = self.chats.entry(chat).or_default();
        while q
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(5))
        {
            q.pop_front();
        }
        q.push_back((now, message));
        if q.len() >= 10 {
            q.drain(..).map(|(_, id)| id).collect()
        } else {
            Vec::new()
        }
    }
}
