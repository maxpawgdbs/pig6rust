mod admin;
mod commands;
mod quote;
mod shadow_handlers;
use crate::{
    ai::Jarvis,
    certificates::{Certificates, signed_role},
    config::{self, JsonStore, Settings, id, list, s},
    economy::Economy,
    moderation::{self, Flood},
    shadow,
    telegram::Telegram,
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
#[derive(Clone)]
pub struct App {
    pub settings: Settings,
    pub config: JsonStore,
    pub shadow: JsonStore,
    pub economy: Arc<Economy>,
    pub certificates: Certificates,
    pub dictionary: Value,
}
impl App {
    pub fn new(settings: Settings) -> Result<Self> {
        std::fs::create_dir_all(&settings.data_dir)?;
        let config = settings.config();
        config.transaction(|d| config::validate(d))?;
        let shadow = JsonStore::new(settings.data_dir.join("shadow.json"), shadow::defaults());
        shadow.transaction(|data| shadow::validate(data))?;
        let requests = JsonStore::new(
            settings.data_dir.join("requests.json"),
            json!({"pending":{},"allowed_downloads":[]}),
        );
        let legacy_allowed = settings.data_dir.join("tmp/allowed_messages.json");
        if !requests.path.exists() && legacy_allowed.exists() {
            let old: Value = serde_json::from_slice(&std::fs::read(legacy_allowed)?)?;
            anyhow::ensure!(
                old["messages"].is_array(),
                "Legacy allowed_messages.json must contain a messages array"
            );
            requests.transaction(|d| {
                d["allowed_downloads"] = old["messages"].clone();
                Ok(())
            })?;
        }
        let dictionary = serde_json::from_slice(
            &std::fs::read("dict.json")
                .context("Read dict.json from the project working directory")?,
        )?;
        Ok(Self {
            economy: Arc::new(Economy::open(settings.data_dir.join("pig6economy.db"))?),
            certificates: Certificates::new(&settings.data_dir),
            settings,
            config,
            shadow,
            dictionary,
        })
    }
    fn requests(&self) -> JsonStore {
        JsonStore::new(
            self.settings.data_dir.join("requests.json"),
            json!({"pending":{},"allowed_downloads":[]}),
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Main,
    Cert,
    Logs,
    Jday,
    Ai,
}
impl Kind {
    pub fn token_env(self) -> &'static str {
        match self {
            Self::Main => "TOKEN",
            Self::Cert => "CERT_TOKEN",
            Self::Logs => "LOGS_TOKEN",
            Self::Jday => "JUDGMENT_DAY_TOKEN",
            Self::Ai => "AI_TOKEN",
        }
    }
}
pub struct Bot {
    app: App,
    tg: Telegram,
    kind: Kind,
    me: Value,
    flood: Flood,
    jarvis: Option<Jarvis>,
}
impl Bot {
    pub async fn run(app: App, kind: Kind, token: String) -> Result<()> {
        let tg = Telegram::new(token, &app.settings.telegram_api)?;
        let me = tg.call("getMe", json!({})).await?;
        tracing::info!(?kind,username=%s(&me["username"]),"Bot started");
        tg.call("deleteWebhook", json!({"drop_pending_updates":false}))
            .await?;
        let mut bot = Self {
            jarvis: if kind == Kind::Ai {
                Some(Jarvis::new(&app.settings)?)
            } else {
                None
            },
            app,
            tg,
            kind,
            me,
            flood: Flood::default(),
        };
        if kind == Kind::Main {
            bot.migrate_shadow().await?;
        }
        let offsets = JsonStore::new(bot.app.settings.data_dir.join("offsets.json"), json!({}));
        let key = id(&bot.me["id"]).to_string();
        let mut offset = id(&offsets.read()?[&key]);
        let mut backoff = 1;
        loop {
            let updates=match bot.tg.call("getUpdates",json!({"offset":offset,"timeout":30,"allowed_updates":["message","channel_post","edited_channel_post","callback_query","inline_query"]})).await {Ok(v)=>{backoff=1;v},Err(e)=>{tracing::warn!(?kind,error=%e,"Polling failed; retrying");tokio::time::sleep(Duration::from_secs(backoff)).await;backoff=(backoff*2).min(30);continue}};
            for update in list(&updates) {
                if let Err(e) = bot.handle(update).await {
                    tracing::error!(?kind,update_id=id(&update["update_id"]),error=%e,"Update failed");
                    if update["message"]["chat"]["type"] == "private" {
                        let _=bot.reply(&update["message"],"Не удалось выполнить запрос. Проверьте параметры команды и повторите попытку. /help — список команд.").await;
                    }
                }
                offset = id(&update["update_id"]) + 1;
                offsets.transaction(|d| {
                    d[&key] = json!(offset);
                    Ok(())
                })?;
            }
        }
    }
    async fn reply(&self, msg: &Value, text: &str) -> Result<()> {
        self.tg.send(id(&msg["chat"]["id"]), text).await?;
        Ok(())
    }
    async fn handle(&mut self, update: &Value) -> Result<()> {
        let config = self.app.config.read()?;
        config::validate(&config)?;
        if self.kind == Kind::Cert && !update["inline_query"].is_null() {
            return self.inline(&update["inline_query"], &config).await;
        }
        if !update["callback_query"].is_null() {
            return self.callback(&update["callback_query"]).await;
        }
        // Edits never re-spend quota or validate a signature that has already rotated.
        if !update["edited_channel_post"].is_null() {
            return Ok(());
        }
        let msg = if !update["channel_post"].is_null() {
            &update["channel_post"]
        } else {
            &update["message"]
        };
        if msg.is_null() {
            return Ok(());
        }
        let chat = id(&msg["chat"]["id"]);
        let user = id(&msg["from"]["id"]);
        let private = msg["chat"]["type"] == "private";
        match self.kind {
            Kind::Jday => {
                if moderation::text(msg) == "/start" {
                    return self
                        .reply(
                            msg,
                            if config["mode"] == "Judgment Day" {
                                "Протокол судного дня активирован"
                            } else {
                                "Протокол судного дня остановлен"
                            },
                        )
                        .await;
                }
                if config["mode"] == "Judgment Day" {
                    let code = s(&config["Judgment Day Code"]);
                    if !code.is_empty() && moderation::text(msg).contains(code) {
                        self.app.config.transaction(|d| {
                            d["Judgment Day Code"] = json!(config::random_hex(16));
                            Ok(())
                        })?;
                    } else {
                        self.tg.delete(chat, id(&msg["message_id"])).await?;
                    }
                }
                return Ok(());
            }
            Kind::Logs => {
                if private && moderation::text(msg) == "/start" {
                    self.app.config.transaction(|d| {
                        let users = d["logs"].as_array_mut().context("logs must be an array")?;
                        if !users.iter().any(|v| id(&v[0]) == user) {
                            users.push(json!([user, format!("@{}", s(&msg["from"]["username"]))]));
                        }
                        Ok(())
                    })?;
                    return self.reply(msg, "Вы подписаны на логи канала.").await;
                }
                if chat == self.app.settings.main_channel && config["logs_mode"] == "on" {
                    for recipient in list(&config["logs"]) {
                        if let Err(e)=self.tg.call("forwardMessage",json!({"chat_id":id(&recipient[0]),"from_chat_id":chat,"message_id":msg["message_id"]})).await {tracing::warn!(error=%e,"Could not forward log");}
                    }
                }
                return Ok(());
            }
            Kind::Cert => return self.cert_command(msg, &config).await,
            Kind::Main => {
                if self.store_message(msg).await? {
                    return Ok(());
                }
                if private && user != 0 {
                    self.app.shadow.transaction(|d| {
                        shadow::ensure_subscription(d, user, shadow::now());
                        Ok(())
                    })?;
                    self.app
                        .economy
                        .add_user(user, msg["from"]["username"].as_str())?;
                }
            }
            Kind::Ai => {}
        }
        if private
            && !msg["document"].is_null()
            && config::root(&config, user, self.app.settings.owner)
        {
            let bytes = self.tg.file(s(&msg["document"]["file_id"])).await?;
            anyhow::ensure!(bytes.len() <= 2_000_000, "Configuration file is too large");
            let replacement: Value = serde_json::from_slice(&bytes)?;
            config::validate(&replacement)?;
            self.app.config.transaction(|d| {
                *d = replacement;
                Ok(())
            })?;
            return self.reply(msg, "Конфигурация обновлена.").await;
        }
        if msg["chat"]["type"] == "channel" {
            if self.kind == Kind::Ai {
                return self.ai_message(msg, &config).await;
            }
            return self.channel(msg, &config).await;
        }
        if self.kind == Kind::Ai {
            return self.ai_message(msg, &config).await;
        }
        if moderation::text(msg).starts_with('/') {
            self.command(msg, &config).await?;
        } else if private {
            self.author(msg).await?;
        }
        Ok(())
    }
    async fn channel(&mut self, msg: &Value, config: &Value) -> Result<()> {
        let chat = id(&msg["chat"]["id"]);
        let message = id(&msg["message_id"]);
        if chat != self.app.settings.main_channel {
            return self.registration(msg, config).await;
        }
        if self.shadow_post(msg, config).await? {
            return Ok(());
        }
        let role = signed_role(&self.app.certificates, config, msg);
        let identity = self.identity(msg, config).await?;
        if identity == "rejected" {
            return Ok(());
        }
        if role.contains("root") || identity == "root" {
            self.root_command(msg, config).await?;
            return Ok(());
        }
        if role.contains("sudo") || identity == "trust" {
            return Ok(());
        }
        let flooded = self.flood.record(chat, message);
        if !flooded.is_empty() {
            self.app.config.transaction(|d| {
                d["ban_messages"] = json!("all");
                Ok(())
            })?;
            for mid in flooded {
                if let Err(e) = self.tg.delete(chat, mid).await {
                    tracing::warn!(error=%e,"Flood cleanup failed");
                }
            }
            return Ok(());
        }
        let mut blocked = moderation::blocked_content(msg, config, &self.app.dictionary)
            || moderation::banned_identity(msg, config);
        if config["mode"] == "Judgment Day" {
            blocked = true
        }
        if !blocked && self.app.economy.redeem(moderation::text(msg))? {
            return Ok(());
        }
        let known = identity == "user" || role.contains("user");
        let mut whitelisted = false;
        if config["white_lists_mode"] == "admins" && !known {
            blocked = true
        }
        if config["white_lists_mode"] == "manual" && !known {
            let allowed = list(&config["white_list"]).iter().any(|v| {
                v == &msg["author_signature"] || (id(v) != 0 && id(v) == id(&msg["from"]["id"]))
            });
            whitelisted = allowed;
            if !allowed {
                blocked = true
            }
        }
        if !known && !whitelisted {
            blocked = true
        }
        if blocked {
            self.tg.delete(chat, message).await?;
        }
        Ok(())
    }
    async fn identity(&self, msg: &Value, config: &Value) -> Result<&'static str> {
        let sig = s(&msg["author_signature"]);
        if sig.is_empty() {
            return Ok("");
        }
        let owner = s(&config["owner_name"]);
        let mut candidates = Vec::new();
        if !owner.is_empty() {
            candidates.push(("owner",0,json!({"name":owner,"uuid":config["uuid"],"channel_id":self.app.settings.personal_channel})));
        }
        for category in ["alpha_users", "admins"] {
            for (index, entry) in list(&config[category]).iter().enumerate() {
                candidates.push((category, index, entry.clone()));
            }
        }
        candidates.sort_by_key(|(_, _, entry)| {
            std::cmp::Reverse(sig == format!("{}{}", s(&entry["name"]), s(&entry["uuid"])))
        });
        for (category, index, entry) in candidates {
            let name = s(&entry["name"]);
            if name.is_empty() || !sig.starts_with(name) {
                continue;
            }
            let valid = sig == format!("{}{}", name, s(&entry["uuid"]));
            if !valid || entry["mute"] == true || entry["EXCOMMUNICADO"] == true {
                self.tg
                    .delete(id(&msg["chat"]["id"]), id(&msg["message_id"]))
                    .await?;
                return Ok("rejected");
            }
            if category == "owner" || category == "alpha_users" || entry["protected"] == true {
                let nonce = config::invisible_id();
                // Consume nonce before network I/O to fail closed on timeouts.
                self.app.config.transaction(|d| {
                    if category == "owner" {
                        d["uuid"] = json!(nonce)
                    } else {
                        d[category][index]["uuid"] = json!(nonce)
                    }
                    Ok(())
                })?;
                if let Err(e) = self
                    .tg
                    .title(id(&entry["channel_id"]), &format!("{name}{nonce}"))
                    .await
                {
                    tracing::error!(error=%e,"Identity rotation failed");
                    self.tg
                        .delete(id(&msg["chat"]["id"]), id(&msg["message_id"]))
                        .await?;
                    return Ok("rejected");
                }
            }
            return Ok(if category == "owner" || category == "alpha_users" {
                "root"
            } else if entry["trust"] == true {
                "trust"
            } else {
                "user"
            });
        }
        Ok("")
    }
    async fn ai_message(&mut self, msg: &Value, config: &Value) -> Result<()> {
        let private = msg["chat"]["type"] == "private";
        let voice = msg["voice"].as_object().is_some();
        if !private && !voice && !crate::ai::addressed(msg, id(&self.me["id"])) {
            return Ok(());
        }
        let text = if voice {
            let bytes = self.tg.file(s(&msg["voice"]["file_id"])).await?;
            self.jarvis
                .as_ref()
                .unwrap()
                .transcribe(
                    bytes,
                    msg["voice"]["mime_type"].as_str().unwrap_or("audio/ogg"),
                )
                .await?
        } else {
            moderation::text(msg).to_string()
        };
        let user = id(&msg["from"]["id"]);
        if config::root(config, user, self.app.settings.owner) {
            let action = self.jarvis.as_ref().unwrap().detect(&text).await?;
            if self.ai_action(&action).await? {
                self.reply(msg, "Команда выполнена.").await?;
                return Ok(());
            }
        }
        let response = self
            .jarvis
            .as_mut()
            .unwrap()
            .query(id(&msg["chat"]["id"]), &text, s(&config["base_prompt"]))
            .await?;
        for chunk in response.chars().collect::<Vec<_>>().chunks(4000) {
            self.reply(msg, &chunk.iter().collect::<String>()).await?;
        }
        Ok(())
    }
    async fn ai_action(&self, action: &Value) -> Result<bool> {
        let name = s(&action["action"]);
        let target = s(&action["target"]);
        let (key, value) = match name {
            "blockall" => ("ban_messages", "all"),
            "disable" => ("ban_messages", "off"),
            "smart" => ("ban_messages", "manual"),
            "logs_on" => ("logs_mode", "on"),
            "logs_off" => ("logs_mode", "off"),
            "jday_on" => ("mode", "Judgment Day"),
            "jday_off" => ("mode", "normal"),
            "setbaseprompt" => ("base_prompt", target),
            "ban" | "unban" | "whitelist_add" | "whitelist_remove" => {
                if target.is_empty() {
                    return Ok(false);
                }
                let key = if name.starts_with("whitelist") {
                    "white_list"
                } else {
                    "banned_users"
                };
                self.app.config.transaction(|d| {
                    let a = d[key].as_array_mut().context("Expected list")?;
                    if name == "ban" || name == "whitelist_add" {
                        if !a.iter().any(|v| v == target) {
                            a.push(json!(target));
                        }
                    } else {
                        a.retain(|v| v != target);
                    }
                    Ok(())
                })?;
                return Ok(true);
            }
            _ => return Ok(false),
        };
        self.app.config.transaction(|d| {
            d[key] = json!(value);
            Ok(())
        })?;
        Ok(true)
    }
}
#[cfg(test)]
mod tests;
