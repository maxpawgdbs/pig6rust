use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// Cross-process lock and atomic replacement preserve hand-edited JSON and unknown fields.
#[derive(Clone, Debug)]
pub struct JsonStore {
    pub path: PathBuf,
    defaults: Value,
}
impl JsonStore {
    pub fn new(path: impl Into<PathBuf>, defaults: Value) -> Self {
        Self {
            path: path.into(),
            defaults,
        }
    }
    pub fn read(&self) -> Result<Value> {
        let mut data = match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("Invalid JSON: {}", self.path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(e.into()),
        };
        if !data.is_object() {
            bail!("{} must contain a JSON object", self.path.display());
        }
        merge_defaults(&mut data, &self.defaults);
        Ok(data)
    }
    pub fn transaction<T>(&self, f: impl FnOnce(&mut Value) -> Result<T>) -> Result<T> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_extension(format!(
                    "{}lock",
                    self.path
                        .extension()
                        .map(|s| format!("{}.", s.to_string_lossy()))
                        .unwrap_or_default()
                )))?;
        lock.lock_exclusive()?;
        let mut data = self.read()?;
        let result = f(&mut data)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut file, &data)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(&self.path).map_err(|e| e.error)?;
        Ok(result)
    }
}
fn merge_defaults(data: &mut Value, defaults: &Value) {
    if let (Some(data), Some(defaults)) = (data.as_object_mut(), defaults.as_object()) {
        for (k, v) in defaults {
            match data.get_mut(k) {
                Some(existing) if existing.is_object() && v.is_object() => {
                    merge_defaults(existing, v)
                }
                Some(_) => {}
                None => {
                    data.insert(k.clone(), v.clone());
                }
            }
        }
    }
}
pub fn defaults() -> Value {
    json!({
        "Pmin":100,"constA":1,"coeff":0.9,"count":100,"cost":300,
        "ban_messages":"off","owner_name":"","uuid":invisible_id(),
        "white_lists_mode":"manual","anon_enable":0,"mode":"normal","AI mode":"messages",
        "Judgment Day Code":random_hex(16),"base_prompt":"","banned":[],"banned_users":[],
        "white_list":[],"logs_mode":"off","logs":[],"protected_users":[],"admins":[],
        "bad_gifs":[],"root_users":[],"alpha_users":[],"signed_users":{}
    })
}
pub fn invisible_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..5)
        .map(|_| char::from_u32(rng.gen_range(0xE0100..0xE01F0)).unwrap())
        .collect()
}
pub fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut b = vec![0; bytes];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}
pub fn id(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}
pub fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
pub fn list(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub fn root(config: &Value, user: i64, owner: i64) -> bool {
    user != 0 && (user == owner || list(&config["root_users"]).iter().any(|v| id(v) == user))
}
pub fn validate(data: &Value) -> Result<()> {
    ensure_object(data)?;
    for key in [
        "banned",
        "banned_users",
        "white_list",
        "logs",
        "admins",
        "bad_gifs",
        "root_users",
        "alpha_users",
    ] {
        if !data[key].is_array() {
            bail!("{key} must be an array")
        }
    }
    if !data["signed_users"].is_object() {
        bail!("signed_users must be an object")
    }
    for (key, values) in [
        ("ban_messages", vec!["off", "manual", "all"]),
        ("white_lists_mode", vec!["off", "manual", "admins"]),
        ("mode", vec!["normal", "Judgment Day"]),
    ] {
        if !values.contains(&s(&data[key])) {
            bail!("Invalid {key}")
        }
    }
    for key in ["owner_name", "uuid", "Judgment Day Code", "base_prompt"] {
        if !data[key].is_string() {
            bail!("{key} must be text")
        }
    }
    for category in ["admins", "alpha_users"] {
        for entry in list(&data[category]) {
            if !entry.is_object() || !entry["name"].is_string() || !entry["uuid"].is_string() {
                bail!("Invalid identity in {category}")
            }
        }
    }
    Ok(())
}
fn ensure_object(data: &Value) -> Result<()> {
    if !data.is_object() {
        bail!("Configuration must be an object")
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct Settings {
    pub data_dir: PathBuf,
    pub web_dir: PathBuf,
    pub owner: i64,
    pub main_channel: i64,
    pub personal_channel: i64,
    pub website: String,
    pub bind: String,
    pub telegram_api: String,
    pub gemini_model: String,
}
impl Settings {
    pub fn from_env(data_dir: PathBuf, web_dir: PathBuf) -> Result<Self> {
        fn number(name: &str) -> Result<i64> {
            match std::env::var(name) {
                Ok(v) => v
                    .parse()
                    .with_context(|| format!("{name} must be an integer")),
                Err(_) => Ok(0),
            }
        }
        Ok(Self {
            data_dir,
            web_dir,
            owner: number("OWNER_ID")?,
            main_channel: number("MAIN_CHANNEL_ID")?,
            personal_channel: number("PERSONAL_CHANNEL_ID")?,
            website: std::env::var("WEB_SITE")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:2322".into())
                .trim()
                .trim_end_matches('/')
                .into(),
            bind: std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:2322".into()),
            telegram_api: std::env::var("TELEGRAM_API_URL")
                .unwrap_or_else(|_| "https://api.telegram.org".into()),
            gemini_model: std::env::var("GEMINI_MODEL")
                .unwrap_or_else(|_| "gemini-3.1-flash-lite".into()),
        })
    }
    pub fn config(&self) -> JsonStore {
        JsonStore::new(self.data_dir.join("config.json"), defaults())
    }
    pub fn policy_url(&self) -> String {
        format!("{}/security-policy", self.website)
    }
}
