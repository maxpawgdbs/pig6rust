use super::*;
use crate::telegram::{button, command, keyboard};
impl Bot {
    pub(super) async fn command(&mut self, msg: &Value, config: &Value) -> Result<()> {
        let (cmd, args) = command(moderation::text(msg));
        let user = id(&msg["from"]["id"]);
        let chat = id(&msg["chat"]["id"]);
        let private = msg["chat"]["type"] == "private";
        let admin = config::root(config, user, self.app.settings.owner);
        if [
            "/apikey", "/mykeys", "/config", "/mycodes", "/jcode", "/logs", "/sub",
        ]
        .contains(&cmd)
            && !private
        {
            return self
                .reply(
                    msg,
                    "Эта команда доступна только в личной переписке с ботом.",
                )
                .await;
        }
        let restricted = [
            "/blockall",
            "/disable",
            "/smart",
            "/jday",
            "/jcode",
            "/config",
            "/setbaseprompt",
            "/post",
            "/link",
            "/gen",
            "/reset",
            "/set",
            "/panel",
            "/logs",
        ];
        if restricted.contains(&cmd) && !admin {
            return self.reply(msg, "Недостаточно прав.").await;
        }
        match cmd {
            "/start"|"/help"=>self.reply(msg,"Свинья-6\n\n/sub — подписка\n/reg — регистрация\n/balance — баланс\n/pay @имя сумма — перевод\n/buy, /sell — рынок кодов\n/mycodes — ваши коды\n/gift — бонус\n/market — рынок\n/top — рейтинг\n/apikey, /mykeys — API\n/panel — управление\n/q — цитата ответом на сообщение\n/download — сохранить сообщение\n/olymp — олимпиады").await?,
            "/sub"=>{self.tg.send_keyboard(chat,&shadow::subscription_view(&self.app.shadow,user)?,Self::sub_keyboard()).await?;},
            "/balance"=>self.reply(msg,&format!("Баланс: {} P6T",self.app.economy.balance(user)?)).await?,
            "/market"=>self.reply(msg,&format!("Рынок кодов\n{}\n{}",self.app.economy.market(config)?,self.app.settings.website)).await?,
            "/top"=>{let top=self.app.economy.top(10)?;let text=list(&top).iter().map(|r|format!("{}. {} — {} P6T",r["rank"],s(&r["name"]),r["balance"])).collect::<Vec<_>>().join("\n");self.reply(msg,&text).await?;},
            "/buy"|"/sell"=>{let count=args.parse::<i64>().unwrap_or(1);anyhow::ensure!((1..=100000).contains(&count),"Неверное количество");let text=format!("{} {count} кодов по текущей рыночной цене?",if cmd=="/buy"{"Купить"}else{"Продать"});let token=self.save_request("trade",json!({"user":user,"count":count,"buy":cmd=="/buy"}))?;self.tg.send_keyboard(chat,&text,keyboard(vec![vec![button("Подтвердить",&format!("trade^{token}"))]])).await?;},
            "/pay"|"/give"=>{let parts:Vec<_>=args.split_whitespace().collect();anyhow::ensure!(parts.len()==2,"Используйте /pay @имя сумма или /give @имя количество кодов");let to=self.app.economy.user_by_name(parts[0])?.context("Пользователь не найден")?;let amount=parts[1].parse()?;if cmd=="/pay" {self.app.economy.transfer(user,to,amount,"user transfer")?;}else{self.app.economy.gift_codes(user,to,amount)?;}self.reply(msg,"Перевод выполнен.").await?;},
            "/gift"=>self.reply(msg,&self.app.economy.bonus(user)?.to_string()).await?,
            "/mycodes"=>self.reply(msg,&format!("Ваши коды:\n{}",self.app.economy.codes(user)?.join("\n"))).await?,
            "/apikey"=>self.reply(msg,&format!("Ваш API-ключ:\n{}\nСохраните его: повторно получить ключ нельзя.",self.app.economy.create_key(user,if args.is_empty(){"Telegram Bot"}else{args})?)).await?,
            "/mykeys"=>{let keys=self.app.economy.keys(user)?;let rows=list(&keys).iter().map(|k|vec![button(&format!("Отозвать {}",s(&k["name"])),&format!("api^revoke^{}",k["id"]))]).collect();self.tg.send_keyboard(chat,"Ваши API-ключи",keyboard(rows)).await?;},
            "/blockall"|"/disable"|"/smart"=>{let mode=match cmd{"/blockall"=>"all","/smart"=>"manual",_=>"off"};self.app.config.transaction(|d|{d["ban_messages"]=json!(mode);Ok(())})?;self.reply(msg,&format!("Режим модерации: {mode}")).await?;},
            "/jday"=>{let mode=self.toggle_jday().await?;self.reply(msg,&format!("Протокол судного дня: {mode}")).await?;},
            "/jcode"=>self.reply(msg,&format!("Код подтверждения: {}",s(&config["Judgment Day Code"]))).await?,
            "/setbaseprompt"=>{self.app.config.transaction(|d|{d["base_prompt"]=json!(args);Ok(())})?;self.reply(msg,"Базовый промпт обновлён.").await?;},
            "/post"=>{self.tg.send(self.app.settings.main_channel,args).await?;self.reply(msg,"Пост отправлен.").await?;},
            "/link"=>{let invite=self.tg.call("createChatInviteLink",json!({"chat_id":self.app.settings.main_channel,"member_limit":1,"expire_date":chrono::Utc::now().timestamp()+600})).await?;self.reply(msg,s(&invite["invite_link"])).await?;},
            "/gen"=>{self.app.economy.generate(args.parse().unwrap_or(id(&config["count"])))?;self.reply(msg,"Коды созданы.").await?;},
            "/reset"=>{let n=self.app.economy.reset_codes()?;self.reply(msg,&format!("Удалено нераспределённых кодов: {n}")).await?;},
            "/set" => {
                let (key, value) = args.split_once(' ').context("Используйте /set параметр значение")?;
                anyhow::ensure!(["Pmin", "constA", "coeff", "count", "cost"].contains(&key), "Неизвестный параметр");
                let value: Value = serde_json::from_str(value)?;
                anyhow::ensure!(value.is_number() && value.as_f64().is_some_and(|n| n >= 0.0), "Нужно неотрицательное число");
                if key == "count" {
                    anyhow::ensure!(value.as_i64().is_some_and(|n| (1..=100000).contains(&n)), "Неверная ёмкость рынка");
                } else if key == "coeff" {
                    anyhow::ensure!(value.as_f64().unwrap() <= 1.0, "coeff должен быть от 0 до 1");
                }
                self.app.config.transaction(|d| { d[key] = value; Ok(()) })?;
                self.reply(msg, "Параметр обновлён.").await?;
            },
            "/reg"=>{let entry=json!({"name":s(&msg["from"]["first_name"]),"uuid":"","owner":format!("@{}",s(&msg["from"]["username"])),"id":user.to_string(),"mute":false,"EXCOMMUNICADO":false,"protected":false,"trust":false});self.request("protect",entry,config,"Запрос на регистрацию пользователя").await?;self.reply(msg,"Запрос отправлен.").await?;},
            "/myaccess"=>self.reply(msg,if admin{"Уровень доступа: ALPHA"}else if list(&config["admins"]).iter().any(|e|id(&e["id"])==user && e["trust"]==true){"Уровень доступа: TRUST"}else{"Уровень доступа: CIVIL"}).await?,
            "/panel"=>self.panel(msg,config).await?,
            "/config"=>{self.tg.upload("sendDocument",chat,"document","config.json",serde_json::to_vec_pretty(config)?).await?;},
            "/logs"=>{let path=self.app.settings.data_dir.join("logs/main.log");if path.exists(){self.tg.upload("sendDocument",chat,"document","main.log",std::fs::read(path)?).await?;}else{self.reply(msg,"Журнал выводится в stdout. Для Docker: docker compose logs.").await?;}},
            "/download"|"/d"=>self.download(msg,config).await?,
            "/q"|"/quote"=>self.quote(msg).await?,
            "/olymp"=>self.olymp(msg,None).await?,
            _=>self.reply(msg,"Неизвестная команда. Список команд: /help").await?,
        }
        Ok(())
    }
    pub(super) async fn cert_command(&self, msg: &Value, config: &Value) -> Result<()> {
        let (cmd, args) = command(moderation::text(msg));
        let user = id(&msg["from"]["id"]);
        if msg["chat"]["type"] != "private" {
            return Ok(());
        }
        if cmd == "/start" {
            if config["signed_users"].get(user.to_string()).is_none() {
                return self
                    .reply(
                        msg,
                        "Вы не зарегистрированы в системе подписей. Обратитесь к администратору.",
                    )
                    .await;
            }
            self.app.certificates.generate(user)?;
            return self
                .reply(
                    msg,
                    "Ключ подписи готов. Используйте бота в inline-режиме, чтобы подписать текст.",
                )
                .await;
        }
        if cmd == "/reg" {
            anyhow::ensure!(!args.is_empty(), "Укажите название канала: /reg название");
            self.request(
                "signed",
                json!({"id":user,"name":args,"username":msg["from"]["username"]}),
                config,
                &format!("Запрос на привязку подписанного канала: {args}"),
            )
            .await?;
            self.reply(msg, "Запрос отправлен.").await?;
        }
        Ok(())
    }
    pub(super) async fn inline(&self, query: &Value, config: &Value) -> Result<()> {
        let text = s(&query["query"]);
        if text.is_empty() {
            return Ok(());
        }
        let user = id(&query["from"]["id"]);
        if config["signed_users"].get(user.to_string()).is_none() {
            self.tg.call("answerInlineQuery",json!({"inline_query_id":query["id"],"results":[],"cache_time":0,"is_personal":true})).await?;
            return Ok(());
        }
        let hidden = s(&config["signed_users"][user.to_string()]["role"]).contains("shadow");
        let cert = self.app.certificates.create(&query["from"], text, hidden)?;
        let url = if hidden {
            format!(
                "{}/shadow?signature={}",
                self.app.settings.website,
                s(&cert["signature"])
            )
        } else {
            format!(
                "{}/check?signature={}&user_id={user}",
                self.app.settings.website,
                s(&cert["signature"])
            )
        };
        let full = format!("{text}{}", crate::certificates::SUFFIX);
        let offset = full.encode_utf16().count() - "View signature.".encode_utf16().count();
        self.tg.call("answerInlineQuery",json!({"inline_query_id":query["id"],"cache_time":0,"is_personal":true,"results":[{"type":"article","id":config::random_hex(8),"title":"Опубликовать подписанное сообщение","input_message_content":{"message_text":full,"entities":[{"type":"text_link","offset":offset,"length":14,"url":url}],"link_preview_options":{"is_disabled":true}}}]})).await?;
        Ok(())
    }
    async fn download(&self, msg: &Value, config: &Value) -> Result<()> {
        let original = &msg["reply_to_message"];
        if original.is_null() {
            return self
                .reply(
                    msg,
                    "Ответьте командой /download на сообщение, которое хотите сохранить.",
                )
                .await;
        }
        let user = id(&msg["from"]["id"]);
        let key = format!("{}{}", id(&msg["chat"]["id"]), id(&original["message_id"]));
        let requests = self.app.requests().read()?;
        if config::root(config, user, self.app.settings.owner)
            || list(&requests["allowed_downloads"])
                .iter()
                .any(|v| v == &key)
        {
            self.deliver(user, original).await?;
            self.reply(msg, "Сообщение отправлено Вам в личные сообщения.")
                .await?;
        } else {
            self.request(
                "download",
                json!({"user":user,"key":key,"message":original}),
                config,
                "Запрос на сохранение сообщения",
            )
            .await?;
            self.reply(msg, "Запрос на сохранение отправлен администратору.")
                .await?;
        }
        Ok(())
    }
    pub(super) async fn deliver(&self, user: i64, msg: &Value) -> Result<()> {
        if let Some(text) = msg["text"].as_str() {
            self.tg.call("sendMessage",json!({"chat_id":user,"text":text,"entities":msg["entities"].as_array().cloned().unwrap_or_default()})).await?;
            return Ok(());
        }
        for (field, method) in [
            ("animation", "sendAnimation"),
            ("photo", "sendPhoto"),
            ("video", "sendVideo"),
            ("sticker", "sendSticker"),
            ("video_note", "sendVideoNote"),
            ("voice", "sendVoice"),
            ("audio", "sendAudio"),
            ("document", "sendDocument"),
        ] {
            let media = if field == "photo" {
                msg[field]
                    .as_array()
                    .and_then(|a| a.last())
                    .unwrap_or(&Value::Null)
            } else {
                &msg[field]
            };
            if let Some(file) = media["file_id"].as_str() {
                let mut body = json!({"chat_id":user});
                body[field] = json!(file);
                if !["sticker", "video_note"].contains(&field) && msg["caption"].is_string() {
                    body["caption"] = msg["caption"].clone();
                    if msg["caption_entities"].is_array() {
                        body["caption_entities"] = msg["caption_entities"].clone();
                    }
                }
                self.tg.call(method, body).await?;
                return Ok(());
            }
        }
        anyhow::bail!("Этот тип сообщения не поддерживается")
    }
    pub(super) async fn olymp(&self, msg: &Value, edit: Option<&str>) -> Result<()> {
        let data: Value = serde_json::from_slice(&std::fs::read("olymps.json")?)?;
        let text = list(&data["list"])
            .iter()
            .map(|o| {
                let events = list(&o["events"])
                    .iter()
                    .map(|e| {
                        format!(
                            "{}: {} ({})",
                            s(&e["name"]),
                            s(&e["date"]),
                            s(&e["location"])
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("{}\n{}\n{events}", s(&o["name"]), s(&o["website"]))
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let _ = edit;
        self.reply(msg, &text).await
    }
}
