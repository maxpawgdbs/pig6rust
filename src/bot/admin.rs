use super::*;
use crate::telegram::{button, keyboard};
impl Bot {
    pub(super) fn save_request(&self, kind: &str, body: Value) -> Result<String> {
        let token = config::random_hex(8);
        self.app.requests().transaction(|d| {
            d["pending"][&token] =
                json!({"kind":kind,"body":body,"created_at":chrono::Utc::now().timestamp()});
            Ok(())
        })?;
        Ok(token)
    }
    pub(super) async fn request(
        &self,
        kind: &str,
        body: Value,
        config: &Value,
        text: &str,
    ) -> Result<()> {
        let token = self.save_request(kind, body)?;
        let kb = keyboard(vec![vec![
            button("Принять", &format!("approve^{token}")),
            button("Отклонить", &format!("reject^{token}")),
        ]]);
        let mut users = vec![self.app.settings.owner];
        users.extend(list(&config["root_users"]).iter().map(id));
        users.sort();
        users.dedup();
        for user in users {
            if user != 0 {
                if let Err(e) = self.tg.send_keyboard(user, text, kb.clone()).await {
                    tracing::warn!(user,error=%e,"Could not send approval request");
                }
            }
        }
        Ok(())
    }
    pub(super) async fn panel(&self, msg: &Value, config: &Value) -> Result<()> {
        let kb = keyboard(vec![
            vec![
                button("Блокировать всё", "mode^all"),
                button("Фильтр", "mode^manual"),
                button("Отключить", "mode^off"),
            ],
            vec![button("Судный день", "jday"), button("Логи", "logs")],
            vec![
                button("Protected", "category^admins"),
                button("Super", "category^alpha_users"),
                button("Signed", "category^signed_users"),
            ],
        ]);
        self.tg
            .send_keyboard(
                id(&msg["chat"]["id"]),
                &format!(
                    "Панель управления\nМодерация: {}\nПротокол: {}\nЛоги: {}",
                    s(&config["ban_messages"]),
                    s(&config["mode"]),
                    s(&config["logs_mode"])
                ),
                kb,
            )
            .await?;
        Ok(())
    }
    pub(super) async fn toggle_jday(&self) -> Result<String> {
        let mode = self.app.config.transaction(|d| {
            let mode = if d["mode"] == "normal" {
                "Judgment Day"
            } else {
                "normal"
            };
            d["mode"] = json!(mode);
            if s(&d["Judgment Day Code"]).is_empty() {
                d["Judgment Day Code"] = json!(config::random_hex(16));
            }
            Ok(mode.to_string())
        })?;
        self.tg
            .send(
                self.app.settings.main_channel,
                &format!("Протокол судного дня: {mode}"),
            )
            .await?;
        Ok(mode)
    }
    pub(super) async fn excommunicate(&self, target: &str) -> Result<()> {
        let (name, channel) = self.app.config.transaction(|d| {
            let entry = list(&d["admins"])
                .iter()
                .find(|e| !s(&e["name"]).is_empty() && target.starts_with(s(&e["name"])))
                .context("Защищённый пользователь не найден")?;
            let result = (s(&entry["name"]).to_string(), id(&entry["channel_id"]));
            d["mode"] = json!("Judgment Day");
            if s(&d["Judgment Day Code"]).is_empty() {
                d["Judgment Day Code"] = json!(config::random_hex(16));
            }
            Ok(result)
        })?;
        for remaining in (1..=5).rev() {
            let code = self.app.config.read()?["Judgment Day Code"].clone();
            if let Err(e) = self
                .tg
                .send(
                    self.app.settings.main_channel,
                    &format!(
                        "{name} EXCOMMUNICADO {remaining}\n\nКод подтверждения: {}",
                        s(&code)
                    ),
                )
                .await
            {
                tracing::warn!(error=%e,"Could not announce excommunication");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        self.app.config.transaction(|d| {
            for entry in d["admins"].as_array_mut().context("admins must be list")? {
                if s(&entry["name"]) == name {
                    entry["EXCOMMUNICADO"] = json!(true);
                    entry["trust"] = json!(false);
                    entry["uuid"] = json!(config::invisible_id());
                }
            }
            d["mode"] = json!("normal");
            Ok(())
        })?;
        if channel != 0 {
            let _ = self
                .tg
                .send(
                    channel,
                    "Вы объявлены EXCOMMUNICADO. Защита канала отозвана.",
                )
                .await;
        }
        self.tg
            .send(
                self.app.settings.main_channel,
                &format!(
                    "{name} EXCOMMUNICADO в силе. Защита отозвана. Протокол судного дня остановлен."
                ),
            )
            .await?;
        Ok(())
    }
    pub(super) async fn root_command(&self, msg: &Value, config: &Value) -> Result<()> {
        let text = moderation::text(msg);
        let (cmd, args) = crate::telegram::command(text);
        match cmd {
            "/ban" | "/unban" => {
                let target = if args.is_empty() {
                    s(&msg["reply_to_message"]["author_signature"])
                } else {
                    args
                };
                if !target.is_empty() {
                    self.app.config.transaction(|d| {
                        let users = d["banned_users"]
                            .as_array_mut()
                            .context("banned_users must be list")?;
                        if cmd == "/ban" {
                            if !users.iter().any(|v| v == target) {
                                users.push(json!(target));
                            }
                        } else {
                            users.retain(|v| v != target);
                        }
                        Ok(())
                    })?;
                }
            }
            "/bangif" => {
                let gif = s(&msg["reply_to_message"]["animation"]["file_id"]);
                if !gif.is_empty() {
                    self.app.config.transaction(|d| {
                        d["bad_gifs"]
                            .as_array_mut()
                            .context("bad_gifs must be list")?
                            .push(json!(gif));
                        Ok(())
                    })?;
                }
            }
            "blockall" | "/blockall" | "disable" | "/disable" | "smart" | "/smart" => {
                let mode = if cmd.contains("blockall") {
                    "all"
                } else if cmd.contains("smart") {
                    "manual"
                } else {
                    "off"
                };
                self.app.config.transaction(|d| {
                    d["ban_messages"] = json!(mode);
                    Ok(())
                })?;
            }
            "EXCOMMUNICADO" => {
                let target = if args.is_empty() {
                    s(&msg["reply_to_message"]["author_signature"])
                } else {
                    args
                };
                self.excommunicate(target).await?;
            }
            _ => {}
        }
        if config["mode"] == "Judgment Day"
            && !s(&config["Judgment Day Code"]).is_empty()
            && text.contains(s(&config["Judgment Day Code"]))
        {
            self.app.config.transaction(|d| {
                d["mode"] = json!("normal");
                d["Judgment Day Code"] = json!(config::random_hex(16));
                Ok(())
            })?;
        }
        Ok(())
    }
    pub(super) async fn callback(&mut self, q: &Value) -> Result<()> {
        let result = self.callback_action(q).await;
        let body = match &result {
            Ok(_) => json!({"callback_query_id":q["id"]}),
            Err(e) => {
                json!({"callback_query_id":q["id"],"text":e.to_string().chars().take(180).collect::<String>(),"show_alert":true})
            }
        };
        self.tg.call("answerCallbackQuery", body).await?;
        result
    }
    async fn callback_action(&mut self, q: &Value) -> Result<()> {
        let config = self.app.config.read()?;
        let data = s(&q["data"]);
        let user = id(&q["from"]["id"]);
        let parts: Vec<_> = data.split('^').collect();
        let msg = &q["message"];
        if data.starts_with("sub^") {
            return self.subscription_callback(q).await;
        }
        if parts.first() == Some(&"api") {
            anyhow::ensure!(
                msg["chat"]["type"] == "private" && id(&msg["chat"]["id"]) == user,
                "Откройте /mykeys в личной переписке"
            );
            let key = parts.get(2).context("Missing key")?.parse()?;
            self.app.economy.revoke(user, key)?;
            self.reply(msg, "Ключ отозван.").await?;
            return Ok(());
        }
        if parts.first() == Some(&"trade") {
            let token = *parts.get(1).context("Missing request")?;
            let requests = self.app.requests();
            let all = requests.read()?;
            let request = &all["pending"][token];
            anyhow::ensure!(
                request["kind"] == "trade" && id(&request["body"]["user"]) == user,
                "Запрос недоступен"
            );
            anyhow::ensure!(
                chrono::Utc::now().timestamp() - id(&request["created_at"]) < 600,
                "Запрос истёк"
            );
            // Claim the confirmation before the trade; double taps cannot execute twice.
            let body = requests.transaction(|d| {
                let request = d["pending"]
                    .as_object_mut()
                    .context("pending must be object")?
                    .remove(token)
                    .context("Запрос уже обработан")?;
                Ok(request["body"].clone())
            })?;
            let result =
                self.app
                    .economy
                    .trade(user, id(&body["count"]), body["buy"] == true, &config)?;
            self.reply(msg, &result.to_string()).await?;
            return Ok(());
        }
        anyhow::ensure!(
            config::root(&config, user, self.app.settings.owner),
            "Недостаточно прав"
        );
        match parts[0] {
            "mode" => {
                let mode = *parts.get(1).context("Missing mode")?;
                anyhow::ensure!(["all", "manual", "off"].contains(&mode), "Invalid mode");
                self.app.config.transaction(|d| {
                    d["ban_messages"] = json!(mode);
                    Ok(())
                })?;
                self.reply(msg, &format!("Режим модерации: {mode}")).await?;
            }
            "jday" => {
                self.toggle_jday().await?;
            }
            "logs" => {
                self.app.config.transaction(|d| {
                    d["logs_mode"] = json!(if d["logs_mode"] == "on" { "off" } else { "on" });
                    Ok(())
                })?;
                self.reply(msg, "Режим логирования изменён.").await?;
            }
            "approve" | "reject" => {
                let token = *parts.get(1).context("Missing request")?;
                let requests = self.app.requests();
                let request = requests.read()?["pending"][token].clone();
                anyhow::ensure!(!request.is_null(), "Запрос уже обработан");
                if parts[0] == "approve" {
                    let body = &request["body"];
                    match s(&request["kind"]) {
                        "protect" => {
                            if body["protected"] == true {
                                self.tg
                                    .title(
                                        id(&body["channel_id"]),
                                        &format!("{}{}", s(&body["name"]), s(&body["uuid"])),
                                    )
                                    .await?;
                            }
                            self.app.config.transaction(|d| {
                                let a =
                                    d["admins"].as_array_mut().context("admins must be list")?;
                                anyhow::ensure!(
                                    !a.iter().any(|e| body["protected"] == true
                                        && e["channel_id"] == body["channel_id"]),
                                    "Канал уже зарегистрирован"
                                );
                                a.push(body.clone());
                                Ok(())
                            })?;
                            if id(&body["id"]) != 0 {
                                self.tg.call("promoteChatMember",json!({"chat_id":self.app.settings.main_channel,"user_id":id(&body["id"]),"can_post_messages":true})).await?;
                            }
                        }
                        "signed" => {
                            let channel = s(&body["name"]);
                            let target = id(&body["id"]).to_string();
                            self.app.config.transaction(|d|{if d["signed_users"].get(&target).is_none(){d["signed_users"][&target]=json!({"username":body["username"],"role":"user","channels":[],"shadow":[]});}let entry=&mut d["signed_users"][&target];if entry["channels"].is_null(){entry["channels"]=json!([]);}let channels=entry["channels"].as_array_mut().context("channels must be list")?;if !channels.iter().any(|c|c==channel){channels.push(json!(channel));}Ok(())})?;
                            let _=self.tg.send(id(&body["id"]),"Ваша заявка Pig-6 Certificates одобрена. Откройте /start для создания ключа.").await;
                        }
                        "download" => {
                            self.deliver(id(&body["user"]), &body["message"]).await?;
                            requests.transaction(|d| {
                                d["allowed_downloads"]
                                    .as_array_mut()
                                    .context("allowed_downloads must be list")?
                                    .push(body["key"].clone());
                                Ok(())
                            })?;
                        }
                        "quote" => {
                            self.deliver_quote(id(&body["user"]), &body["message"])
                                .await?;
                            requests.transaction(|d| {
                                d["allowed_downloads"]
                                    .as_array_mut()
                                    .context("allowed_downloads must be list")?
                                    .push(body["key"].clone());
                                Ok(())
                            })?;
                        }
                        _ => anyhow::bail!("Неизвестный тип запроса"),
                    }
                }
                requests.transaction(|d| {
                    d["pending"].as_object_mut().unwrap().remove(token);
                    Ok(())
                })?;
                self.tg.call("editMessageText",json!({"chat_id":msg["chat"]["id"],"message_id":msg["message_id"],"text":if parts[0]=="approve"{"Запрос принят."}else{"Запрос отклонён."}})).await?;
            }
            "category" => {
                let category = *parts.get(1).context("Missing category")?;
                anyhow::ensure!(
                    ["admins", "alpha_users", "signed_users"].contains(&category),
                    "Invalid category"
                );
                let mut rows = Vec::new();
                if category == "signed_users" {
                    for (key, entry) in config[category].as_object().context("Expected object")? {
                        rows.push(vec![button(
                            &format!("{}: {}", key, s(&entry["role"])),
                            &format!("signed^{key}"),
                        )]);
                    }
                } else {
                    for (index, entry) in list(&config[category]).iter().enumerate() {
                        rows.push(vec![button(
                            s(&entry["name"]),
                            &format!("user^{category}^{index}"),
                        )]);
                    }
                }
                self.tg
                    .send_keyboard(id(&msg["chat"]["id"]), category, keyboard(rows))
                    .await?;
            }
            "user" => {
                let category = *parts.get(1).context("Missing category")?;
                anyhow::ensure!(
                    ["admins", "alpha_users"].contains(&category),
                    "Invalid category"
                );
                let index: usize = parts.get(2).context("Missing index")?.parse()?;
                let entry = config[category].get(index).context("User not found")?;
                self.tg
                    .send_keyboard(
                        id(&msg["chat"]["id"]),
                        &serde_json::to_string_pretty(entry)?,
                        keyboard(vec![vec![
                            button("Mute", &format!("toggle^{category}^{index}^mute")),
                            button("Trust", &format!("toggle^{category}^{index}^trust")),
                            button(
                                "EXCOMMUNICADO",
                                &format!("toggle^{category}^{index}^EXCOMMUNICADO"),
                            ),
                        ]]),
                    )
                    .await?;
            }
            "toggle" => {
                let category = *parts.get(1).context("Missing category")?;
                let index: usize = parts.get(2).context("Missing index")?.parse()?;
                let field = *parts.get(3).context("Missing field")?;
                anyhow::ensure!(
                    ["admins", "alpha_users"].contains(&category)
                        && ["mute", "trust", "EXCOMMUNICADO"].contains(&field),
                    "Invalid action"
                );
                if field == "EXCOMMUNICADO" && config[category][index][field] != true {
                    self.excommunicate(s(&config[category][index]["name"]))
                        .await?;
                    return Ok(());
                }
                self.app.config.transaction(|d| {
                    let entry = d[category].get_mut(index).context("User not found")?;
                    entry[field] = json!(entry[field] != true);
                    Ok(())
                })?;
                self.reply(msg, "Настройка изменена.").await?;
            }
            "signed" => {
                let target = *parts.get(1).context("Missing user")?;
                let entry = &config["signed_users"][target];
                anyhow::ensure!(!entry.is_null(), "User not found");
                let roles = [
                    "user",
                    "sudo",
                    "root",
                    "user shadow",
                    "sudo shadow",
                    "root shadow",
                ];
                let mut rows: Vec<Vec<Value>> = roles
                    .iter()
                    .map(|role| vec![button(role, &format!("role^{target}^{role}"))])
                    .collect();
                for (index, channel) in list(&entry["channels"]).iter().enumerate() {
                    rows.push(vec![
                        button(
                            &format!("Отозвать {}", s(channel)),
                            &format!("unlink^{target}^{index}"),
                        ),
                        button("Shadow", &format!("channelshadow^{target}^{index}")),
                    ]);
                }
                self.tg
                    .send_keyboard(id(&msg["chat"]["id"]), &entry.to_string(), keyboard(rows))
                    .await?;
            }
            "role" => {
                let target = *parts.get(1).context("Missing user")?;
                let role = *parts.get(2).context("Missing role")?;
                anyhow::ensure!(
                    [
                        "user",
                        "sudo",
                        "root",
                        "user shadow",
                        "sudo shadow",
                        "root shadow"
                    ]
                    .contains(&role),
                    "Invalid role"
                );
                self.app.config.transaction(|d| {
                    anyhow::ensure!(d["signed_users"].get(target).is_some(), "User not found");
                    d["signed_users"][target]["role"] = json!(role);
                    Ok(())
                })?;
                self.reply(msg, "Роль изменена.").await?;
            }
            "unlink" | "channelshadow" => {
                let target = *parts.get(1).context("Missing user")?;
                let index: usize = parts.get(2).context("Missing channel")?.parse()?;
                self.app.config.transaction(|d| {
                    let entry = d["signed_users"]
                        .get_mut(target)
                        .context("User not found")?;
                    let channel = entry["channels"]
                        .get(index)
                        .context("Channel not found")?
                        .clone();
                    if data.starts_with("unlink^") {
                        entry["channels"]
                            .as_array_mut()
                            .context("channels must be list")?
                            .remove(index);
                        if let Some(hidden) = entry["shadow"].as_array_mut() {
                            hidden.retain(|c| c != &channel);
                        }
                    } else {
                        if !entry["shadow"].is_array() {
                            entry["shadow"] = json!([]);
                        }
                        let hidden = entry["shadow"].as_array_mut().unwrap();
                        if hidden.contains(&channel) {
                            hidden.retain(|c| c != &channel);
                        } else {
                            hidden.push(channel);
                        }
                    }
                    Ok(())
                })?;
                self.reply(msg, "Настройки канала изменены.").await?;
            }
            _ => anyhow::bail!("Откройте панель заново: /panel"),
        }
        Ok(())
    }
}
