use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::time::Duration;
#[derive(Clone)]
pub struct Telegram {
    client: reqwest::Client,
    base: String,
    token: String,
}
impl Telegram {
    pub fn new(token: String, base: &str) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(65))
                .build()?,
            base: base.trim_end_matches('/').into(),
            token,
        })
    }
    fn endpoint(&self, method: &str) -> String {
        format!("{}/bot{}/{method}", self.base, self.token)
    }
    pub async fn call(&self, method: &str, body: Value) -> Result<Value> {
        // Strip reqwest URLs from errors: Telegram URLs contain the bot credential.
        let response = self
            .client
            .post(self.endpoint(method))
            .json(&body)
            .send()
            .await
            .map_err(|e| e.without_url())?;
        let status = response.status();
        let data: Value = response.json().await.map_err(|e| e.without_url())?;
        ensure!(
            data["ok"] == true,
            "Telegram {method} ({status}): {}",
            data["description"].as_str().unwrap_or("Unknown error")
        );
        Ok(data["result"].clone())
    }
    pub async fn send(&self, chat: i64, text: &str) -> Result<Value> {
        self.call(
            "sendMessage",
            json!({"chat_id":chat,"text":text,"link_preview_options":{"is_disabled":true}}),
        )
        .await
    }
    pub async fn send_keyboard(&self, chat: i64, text: &str, keyboard: Value) -> Result<Value> {
        self.call("sendMessage",json!({"chat_id":chat,"text":text,"reply_markup":keyboard,"link_preview_options":{"is_disabled":true}})).await
    }
    pub async fn delete(&self, chat: i64, message: i64) -> Result<()> {
        self.call(
            "deleteMessage",
            json!({"chat_id":chat,"message_id":message}),
        )
        .await?;
        Ok(())
    }
    pub async fn title(&self, chat: i64, title: &str) -> Result<()> {
        self.call("setChatTitle", json!({"chat_id":chat,"title":title}))
            .await?;
        Ok(())
    }
    pub async fn upload(
        &self,
        method: &str,
        chat: i64,
        field: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<Value> {
        let part = reqwest::multipart::Part::bytes(bytes).file_name(name.to_string());
        let form = reqwest::multipart::Form::new()
            .text("chat_id", chat.to_string())
            .part(field.to_string(), part);
        let response = self
            .client
            .post(self.endpoint(method))
            .multipart(form)
            .send()
            .await
            .map_err(|e| e.without_url())?;
        let data: Value = response.json().await.map_err(|e| e.without_url())?;
        ensure!(
            data["ok"] == true,
            "Telegram upload {method}: {}",
            data["description"]
        );
        Ok(data["result"].clone())
    }
    pub async fn file(&self, file_id: &str) -> Result<Vec<u8>> {
        let file = self.call("getFile", json!({"file_id":file_id})).await?;
        let path = file["file_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Telegram returned no file path"))?;
        let response = self
            .client
            .get(format!("{}/file/bot{}/{path}", self.base, self.token))
            .send()
            .await
            .map_err(|e| e.without_url())?
            .error_for_status()
            .map_err(|e| e.without_url())?;
        Ok(response
            .bytes()
            .await
            .map_err(|e| e.without_url())?
            .to_vec())
    }
}
pub fn button(text: &str, data: &str) -> Value {
    json!({"text":text,"callback_data":data})
}
pub fn keyboard(rows: Vec<Vec<Value>>) -> Value {
    json!({"inline_keyboard":rows})
}
pub fn command(text: &str) -> (&str, &str) {
    let (first, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    (first.split('@').next().unwrap_or(first), rest.trim())
}
