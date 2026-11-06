use crate::config::{id, random_hex, s};
use anyhow::{Result, ensure};
use ed25519_dalek::{
    Signature, Signer, SigningKey, VerifyingKey,
    pkcs8::{
        DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey,
        spki::der::pem::LineEnding,
    },
};
use rand::RngCore;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub const SUFFIX: &str = "\n\nThis message was signed by Pig-6 Certificates.\nView signature.";

#[derive(Clone)]
pub struct Certificates {
    dir: PathBuf,
}
impl Certificates {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    fn private(&self, user: i64) -> PathBuf {
        self.dir.join(format!("keys/private/{user}.private.pem"))
    }
    fn public(&self, user: i64) -> PathBuf {
        self.dir.join(format!("keys/public/{user}.public.pem"))
    }
    pub fn generate(&self, user: i64) -> Result<()> {
        let path = self.private(user);
        if path.exists() {
            SigningKey::from_pkcs8_pem(&fs::read_to_string(path)?)?;
            return Ok(());
        }
        let mut seed = [0; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let key = SigningKey::from_bytes(&seed);
        fs::create_dir_all(self.dir.join("keys/private"))?;
        fs::create_dir_all(self.dir.join("keys/public"))?;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        use std::io::Write;
        let mut private = opts.open(path)?;
        private.write_all(key.to_pkcs8_pem(LineEnding::LF)?.as_bytes())?;
        private.sync_all()?;
        fs::write(
            self.public(user),
            key.verifying_key().to_public_key_pem(LineEnding::LF)?,
        )?;
        Ok(())
    }
    pub fn sign(&self, user: i64, text: &str) -> Result<String> {
        let key = SigningKey::from_pkcs8_pem(&fs::read_to_string(self.private(user))?)?;
        Ok(hex::encode(
            key.sign(format!("{user}:{text}").as_bytes()).to_bytes(),
        ))
    }
    pub fn verify(&self, user: i64, text: &str, signature: &str) -> bool {
        (|| -> Result<()> {
            let key = VerifyingKey::from_public_key_pem(&fs::read_to_string(self.public(user))?)?;
            let sig = Signature::from_slice(&hex::decode(signature)?)?;
            key.verify_strict(format!("{user}:{text}").as_bytes(), &sig)?;
            Ok(())
        })()
        .is_ok()
    }
    pub fn create(&self, user: &Value, text: &str, shadow: bool) -> Result<Value> {
        let user_id = id(&user["id"]);
        let signature = self.sign(user_id, text)?;
        let cert = json!({"id":random_hex(8),"author":{"id":user_id,"first_name":user["first_name"],"last_name":user["last_name"],"username":format!("@{}",s(&user["username"]))},"message":text,"signature":signature,"shadow":shadow,"created_at":chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string()});
        fs::create_dir_all(self.dir.join("certificates"))?;
        fs::write(
            self.dir.join(format!("certificates/{signature}.json")),
            serde_json::to_vec_pretty(&cert)?,
        )?;
        Ok(cert)
    }
    pub fn read(&self, signature: &str) -> Result<Value> {
        ensure!(
            signature.len() == 128 && signature.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid signature"
        );
        Ok(serde_json::from_slice(&fs::read(
            self.dir.join(format!("certificates/{signature}.json")),
        )?)?)
    }
    pub fn public_pem(&self, user: i64) -> Result<String> {
        Ok(fs::read_to_string(self.public(user))?)
    }
}
pub fn signed_role(certs: &Certificates, config: &Value, msg: &Value) -> String {
    let text = s(&msg["text"]);
    if text.is_empty() {
        return String::new();
    }
    let payload = text.strip_suffix(SUFFIX).unwrap_or(text);
    for entity in crate::config::list(&msg["entities"]) {
        if s(&entity["type"]) != "text_link" {
            continue;
        }
        let Ok(url) = url::Url::parse(s(&entity["url"])) else {
            continue;
        };
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        let Some(signature) = params.get("signature") else {
            continue;
        };
        if let Some(users) = config["signed_users"].as_object() {
            for (user, data) in users {
                let Ok(user_id) = user.parse::<i64>() else {
                    continue;
                };
                let role = s(&data["role"]);
                let direct = params.get("user_id") == Some(user);
                let hidden = role.contains("shadow")
                    && (role.contains("root") || s(&msg["author_signature"]) == user);
                if (direct || hidden) && certs.verify(user_id, payload, signature) {
                    return role.to_string();
                }
            }
        }
    }
    String::new()
}
pub fn policy_hash(web_dir: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    Ok(hex::encode(Sha256::digest(fs::read(
        web_dir.join("security-policy.txt"),
    )?)))
}
