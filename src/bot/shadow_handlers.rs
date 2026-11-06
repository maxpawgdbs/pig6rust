use super::*;
use crate::telegram::{button, keyboard};
impl Bot {
    pub(super) async fn migrate_shadow(&self) -> Result<()> {
        let data = self.app.shadow.read()?;
        shadow::validate(&data)?;
        for (key, channel) in data["channels"]
            .as_object()
            .context("channels must be object")?
        {
            if shadow::invisible(s(&channel["name"])) || channel["enabled"] != true {
                continue;
            }
            let name = shadow::channel_name(&data, None);
            let nonce = config::invisible_id();
            match self
                .tg
                .title(id(&channel["channel_id"]), &format!("{name}{nonce}"))
                .await
            {
                Ok(_) => self.app.shadow.transaction(|d| {
                    d["channels"][key]["name"] = json!(name);
                    d["channels"][key]["uuid"] = json!(nonce);
                    Ok(())
                })?,
                Err(e) => tracing::warn!(channel=%key,error=%e,"Could not migrate Shadow title"),
            }
        }
        Ok(())
    }
    pub(super) async fn store_message(&self, msg: &Value) -> Result<bool> {
        if msg["chat"]["is_direct_messages"] != true {
            return Ok(false);
        }
        let chat = self
            .tg
            .call("getChat", json!({"chat_id":msg["chat"]["id"]}))
            .await?;
        let mut parent = chat["parent_chat"].clone();
        if parent.is_null() {
            return Ok(false);
        }
        if parent["username"].is_null() {
            parent = self
                .tg
                .call("getChat", json!({"chat_id":parent["id"]}))
                .await?;
        }
        let data = self.app.shadow.read()?;
        let plan = data["stores"][s(&parent["username"]).to_lowercase()].clone();
        let Ok(user) = shadow::check_store_user(msg, &plan) else {
            return Ok(false);
        };
        let result = self.app.shadow.transaction(|d| {
            shadow::grant(
                d,
                user,
                s(&plan),
                &format!("{}:{}", id(&msg["chat"]["id"]), id(&msg["message_id"])),
                shadow::now(),
            )
        })?;
        self.tg.call("sendMessage",json!({"chat_id":msg["chat"]["id"],"direct_messages_topic_id":msg["direct_messages_topic"]["topic_id"],"text":format!("Подписка {} подключена.\nДействует до {}.\n\nВаши лимиты — в /sub у бота.",s(&result["plan"]),s(&result["expires_at"]))})).await?;
        Ok(true)
    }
    pub(super) async fn registration(&self, msg: &Value, config: &Value) -> Result<()> {
        let text = moderation::text(msg).trim().to_lowercase();
        let chat = id(&msg["chat"]["id"]);
        if text == "unprotect" {
            self.app.config.transaction(|d| {
                for key in ["admins", "alpha_users"] {
                    if let Some(entries) = d[key].as_array_mut() {
                        entries.retain(|e| id(&e["channel_id"]) != chat);
                    }
                }
                Ok(())
            })?;
            return self.reply(msg, "Защита отключена.").await;
        }
        if text != "shadow" && text != "protect" {
            return Ok(());
        }
        if list(&config["admins"])
            .iter()
            .chain(list(&config["alpha_users"]))
            .any(|c| id(&c["channel_id"]) == chat)
        {
            return self
                .reply(
                    msg,
                    "В канале уже действует обычная защита. Сначала опубликуйте unprotect.",
                )
                .await;
        }
        let admins = self
            .tg
            .call("getChatAdministrators", json!({"chat_id":chat}))
            .await?;
        let humans: Vec<_> = list(&admins)
            .iter()
            .filter(|a| a["user"]["is_bot"] != true)
            .collect();
        let bot = list(&admins)
            .iter()
            .find(|a| id(&a["user"]["id"]) == id(&self.me["id"]));
        if humans.len() != 1
            || !["creator", "owner"].contains(&s(&humans[0]["status"]))
            || list(&admins).len() != 2
            || bot.is_none_or(|b| b["can_change_info"] != true)
        {
            return self.reply(msg,"Оставьте в администраторах только владельца и бота. Разрешите боту менять информацию канала.").await;
        }
        let owner = &humans[0]["user"];
        if text == "protect" {
            let entry = json!({"name":msg["chat"]["title"],"channel_id":chat,"uuid":config::invisible_id(),"owner":format!("@{}",s(&owner["username"])),"id":id(&owner["id"]).to_string(),"mute":false,"EXCOMMUNICADO":false,"trust":false,"protected":true});
            self.request(
                "protect",
                entry,
                config,
                &format!("Запрос на защиту канала: {}", s(&msg["chat"]["title"])),
            )
            .await?;
            return self.reply(msg, "Запрос на защиту отправлен.").await;
        }
        let data = self.app.shadow.read()?;
        let name = shadow::channel_name(&data, data["channels"].get(chat.to_string()));
        let nonce = config::invisible_id();
        self.tg.title(chat, &format!("{name}{nonce}")).await?;
        self.app.shadow.transaction(|d|{d["channels"][chat.to_string()]=json!({"channel_id":chat,"owner":shadow::owner_info(owner),"name":name,"uuid":nonce,"enabled":true});shadow::ensure_subscription(d,id(&owner["id"]),shadow::now());Ok(())})?;
        self.reply(msg,"Анонимные публикации подключены.\n\nПубликуйте в основном канале от имени этого канала. Ваши лимиты — в /sub у бота.").await
    }
    pub(super) async fn shadow_post(&self, msg: &Value, config: &Value) -> Result<bool> {
        let data = self.app.shadow.read()?;
        let sender = id(&msg["sender_chat"]["id"]);
        let chat = id(&msg["chat"]["id"]);
        let message = id(&msg["message_id"]);
        let key = format!("{chat}:{message}");
        let sig = msg["author_signature"]
            .as_str()
            .or_else(|| {
                if sender != self.app.settings.main_channel {
                    msg["sender_chat"]["title"].as_str()
                } else {
                    None
                }
            })
            .unwrap_or("");
        let channels = data["channels"]
            .as_object()
            .context("channels must be object")?;
        let recorded = data["posts"].get(&key);
        let channel = channels
            .get(&sender.to_string())
            .or_else(|| {
                channels
                    .values()
                    .find(|c| !s(&c["name"]).is_empty() && sig.starts_with(s(&c["name"])))
            })
            .or_else(|| recorded.and_then(|p| channels.get(&id(&p["channel_id"]).to_string())));
        let Some(channel) = channel else {
            if sig.starts_with("Shadow ") || (sig.chars().count() == 10 && shadow::invisible(sig)) {
                self.tg.delete(chat, message).await?;
                return Ok(true);
            }
            return Ok(false);
        };
        let album = msg["media_group_id"].as_str();
        let album_post = data["posts"].as_object().unwrap().values().find(|p| {
            album.is_some_and(|a| p["media_group_id"] == a)
                && id(&p["chat_id"]) == chat
                && p["channel_id"] == channel["channel_id"]
                && p["signature"] == sig
        });
        let valid = channel["enabled"] == true
            && (sig == format!("{}{}", s(&channel["name"]), s(&channel["uuid"]))
                || recorded.is_some_and(|r| r["signature"] == sig)
                || album_post.is_some());
        if !valid
            || (sender != 0
                && sender != self.app.settings.main_channel
                && sender != id(&channel["channel_id"]))
            || config["mode"] != "normal"
            || moderation::blocked_content(msg, config, &self.app.dictionary)
        {
            self.tg.delete(chat, message).await?;
            return Ok(true);
        }
        if recorded.is_some() {
            return Ok(true);
        }
        let time = shadow::now();
        let name = shadow::channel_name(&data, Some(channel));
        let nonce = config::invisible_id();
        let channel_id = id(&channel["channel_id"]);
        let result = self.app.shadow.transaction(|d| {
            let result = shadow::reserve(d, channel, chat, message, time, album)?;
            if result == "accepted" {
                if album_post.is_some() {
                    d["posts"][&key]["signature"] = json!(sig)
                } else {
                    d["channels"][channel_id.to_string()]["name"] = json!(name);
                    d["channels"][channel_id.to_string()]["uuid"] = json!(nonce);
                }
            }
            Ok(result)
        })?;
        if result == "limit" {
            self.tg.delete(chat, message).await?;
            let _=self.tg.send(id(&channel["owner"]["id"]),&format!("Публикации на сегодня закончились.\nЛимит обновится через {}.\nИзменить подписку можно в /sub.",shadow::reset_text(time))).await;
            return Ok(true);
        }
        if result == "existing" || album_post.is_some() {
            return Ok(true);
        }
        if let Err(e) = self.tg.title(channel_id, &format!("{name}{nonce}")).await {
            self.app.shadow.transaction(|d| {
                shadow::refund(d, &key, time)?;
                d["channels"][channel_id.to_string()]["enabled"] = json!(false);
                Ok(())
            })?;
            self.tg.delete(chat, message).await?;
            let _=self.tg.send(id(&channel["owner"]["id"]),"Защита канала приостановлена. Проверьте права бота и опубликуйте Shadow ещё раз. Пост удалён, лимит не потрачен.").await;
            tracing::warn!(error=%e,"Shadow nonce rotation failed");
        }
        Ok(true)
    }
    pub(super) async fn author(&self, msg: &Value) -> Result<()> {
        let Some((chat_ref, message)) = shadow::post_link(moderation::text(msg))? else {
            return Ok(());
        };
        let chat = if let Some(raw) = chat_ref.strip_prefix("c/") {
            format!("-100{raw}").parse()?
        } else {
            id(&self
                .tg
                .call("getChat", json!({"chat_id":format!("@{chat_ref}")}))
                .await?["id"])
        };
        if chat != self.app.settings.main_channel {
            return self
                .reply(
                    msg,
                    "Узнать автора можно только у Shadow-поста из основного канала.",
                )
                .await;
        }
        let key = format!("{chat}:{message}");
        let user = id(&msg["from"]["id"]);
        let time = shadow::now();
        let (result, owner) = self
            .app
            .shadow
            .transaction(|d| shadow::reveal(d, user, &key, time))?;
        match result {
            "unknown"=>self.reply(msg,"Автор этого поста неизвестен. Информация сохраняется для новых анонимных публикаций.").await?,
            "limit"=>self.reply(msg,"Нет доступных просмотров авторства. Ваши лимиты — в /sub.").await?,
            _=>{let name=if s(&owner["username"]).is_empty(){s(&owner["name"]).to_string()}else{format!("{} (@{})",s(&owner["name"]),s(&owner["username"]))};
                if let Err(e)=self.reply(msg,&format!("Автор: {name}")).await {if result=="revealed"{self.app.shadow.transaction(|d|{shadow::refund_view(d,user,&key,time);Ok(())})?;}return Err(e)}
            }
        }
        Ok(())
    }
    pub(super) async fn subscription_callback(&self, q: &Value) -> Result<()> {
        let user = id(&q["from"]["id"]);
        let msg = &q["message"];
        anyhow::ensure!(
            msg["chat"]["type"] == "private" && id(&msg["chat"]["id"]) == user,
            "Откройте /sub в личной переписке."
        );
        let action = s(&q["data"]).split('^').nth(1).unwrap_or("");
        let (text, kb) = match action {
            "change" => {
                let text = include_str!("../../assets/subscription-terms.txt")
                    .replace("{SECURITY_POLICY_URL}", &self.app.settings.policy_url());
                (
                    text,
                    keyboard(vec![
                        vec![
                            json!({"text":"Подтверждаю и принимаю","callback_data":"sub^accept","style":"success"}),
                        ],
                        vec![
                            json!({"text":"Политика безопасности","url":self.app.settings.policy_url()}),
                        ],
                        vec![button("К подписке", "sub^back")],
                    ]),
                )
            }
            "accept" => {
                let hash = crate::certificates::policy_hash(&self.app.settings.web_dir)?;
                self.app.shadow.transaction(|d| {
                    let t = shadow::now();
                    let sub = shadow::ensure_subscription(d, user, t);
                    for key in ["terms_accepted_at", "administrator_confirmed_at"] {
                        sub[key] = json!(t.to_rfc3339());
                    }
                    sub["terms_version"] = json!(2);
                    sub["security_policy_url"] = json!(self.app.settings.policy_url());
                    sub["security_policy_sha256"] = json!(hash);
                    Ok(())
                })?;
                let data = self.app.shadow.read()?;
                let mut text = "Выберите подписку\nЛимиты в день: посты / просмотры авторства.\n\n"
                    .to_string();
                for (name, quota) in data["plans"].as_object().unwrap() {
                    text.push_str(&format!(
                        "{name}: {} / {}\n",
                        quota["posts_per_day"], quota["views_per_day"]
                    ));
                }
                text.push_str("\nДля покупки отправьте сообщение магазину. Подписка будет подключена или продлена на месяц. Ultra подключает администратор.");
                let mut buttons = Vec::new();
                for (store, plan) in data["stores"].as_object().unwrap() {
                    buttons.push(vec![json!({"text":plan,"url":format!("https://t.me/{store}?direct"),"style":"primary"})]);
                }
                buttons.push(vec![button("К подписке", "sub^back")]);
                (text, keyboard(buttons))
            }
            _ => (
                shadow::subscription_view(&self.app.shadow, user)?,
                Self::sub_keyboard(),
            ),
        };
        self.tg.call("editMessageText",json!({"chat_id":msg["chat"]["id"],"message_id":msg["message_id"],"text":text,"reply_markup":kb})).await?;
        Ok(())
    }
    pub(super) fn sub_keyboard() -> Value {
        keyboard(vec![vec![
            json!({"text":"Изменить подписку","callback_data":"sub^change","style":"primary"}),
        ]])
    }
}
