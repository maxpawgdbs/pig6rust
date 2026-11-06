use crate::config::{id, random_hex};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Mutex};
pub struct Economy {
    db: Mutex<Connection>,
}
impl Economy {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }
    pub fn memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }
    fn from_connection(db: Connection) -> Result<Self> {
        db.busy_timeout(std::time::Duration::from_secs(10))?;
        db.execute_batch(include_str!("schema.sql"))?;
        db.execute(
            "INSERT OR IGNORE INTO users(user_id,balance,name) VALUES(0,0,'SYSTEM')",
            [],
        )?;
        Ok(Self { db: Mutex::new(db) })
    }
    fn transaction<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    fn add(tx: &Connection, user: i64, name: Option<&str>) -> Result<()> {
        tx.execute(
            "INSERT OR IGNORE INTO users(user_id,balance,name) VALUES(?1,100,?2)",
            params![user, name],
        )?;
        if let Some(name) = name {
            tx.execute(
                "UPDATE users SET name=?2 WHERE user_id=?1 AND (name IS NULL OR name='')",
                params![user, name],
            )?;
        }
        Ok(())
    }
    pub fn add_user(&self, user: i64, name: Option<&str>) -> Result<()> {
        self.transaction(|tx| Self::add(tx, user, name))
    }
    fn balance_tx(tx: &Connection, user: i64) -> Result<i64> {
        Self::add(tx, user, None)?;
        Ok(
            tx.query_row("SELECT balance FROM users WHERE user_id=?1", [user], |r| {
                r.get(0)
            })?,
        )
    }
    pub fn balance(&self, user: i64) -> Result<i64> {
        self.transaction(|tx| Self::balance_tx(tx, user))
    }
    fn transfer_tx(
        tx: &Transaction<'_>,
        from: i64,
        to: i64,
        amount: i64,
        comment: &str,
    ) -> Result<()> {
        ensure!(amount > 0 && from != to, "Invalid transfer");
        Self::add(tx, from, None)?;
        Self::add(tx, to, None)?;
        if from != 0 {
            ensure!(
                tx.execute(
                    "UPDATE users SET balance=balance-?2 WHERE user_id=?1 AND balance>=?2",
                    params![from, amount]
                )? == 1,
                "Insufficient funds"
            );
        }
        tx.execute(
            "UPDATE users SET balance=balance+?2 WHERE user_id=?1",
            params![to, amount],
        )?;
        tx.execute(
            "INSERT INTO transactions(sender,receiver,amount,comment) VALUES(?1,?2,?3,?4)",
            params![from, to, amount, comment],
        )?;
        Ok(())
    }
    pub fn transfer(&self, from: i64, to: i64, amount: i64, comment: &str) -> Result<i64> {
        self.transaction(|tx| {
            Self::transfer_tx(tx, from, to, amount, comment)?;
            Self::balance_tx(tx, from)
        })
    }
    pub fn user_by_name(&self, name: &str) -> Result<Option<i64>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .query_row(
                "SELECT user_id FROM users WHERE lower(ltrim(name,'@'))=lower(ltrim(?1,'@'))",
                [name],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub fn top(&self, limit: i64) -> Result<Value> {
        let db = self.db.lock().unwrap();
        let mut q=db.prepare("SELECT name,balance FROM users WHERE balance>0 AND user_id!=0 ORDER BY balance DESC LIMIT ?1")?;
        let rows = q
            .query_map([limit.clamp(1, 1000)], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!(rows.into_iter().enumerate().map(|(i,(name,balance))|json!({"rank":i+1,"name":name.unwrap_or_else(||"Anonymous".into()).replace('@',""),"balance":balance})).collect::<Vec<_>>()))
    }
    pub fn create_key(&self, user: i64, name: &str) -> Result<String> {
        use base64::Engine;
        let raw = format!(
            "pig6_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hex::decode(random_hex(32))?)
        );
        let hash = hex::encode(Sha256::digest(raw.as_bytes()));
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO api_keys(key_hash,owner_id,name) VALUES(?1,?2,?3)",
                params![hash, user, name],
            )?;
            Ok(())
        })?;
        Ok(raw)
    }
    pub fn authenticate(&self, key: &str) -> Result<Option<i64>> {
        let hash = hex::encode(Sha256::digest(key.as_bytes()));
        self.transaction(|tx| {
            let user = tx
                .query_row(
                    "SELECT owner_id FROM api_keys WHERE key_hash=?1 AND revoked=0",
                    [&hash],
                    |r| r.get(0),
                )
                .optional()?;
            if user.is_some() {
                tx.execute(
                    "UPDATE api_keys SET last_used_at=CURRENT_TIMESTAMP WHERE key_hash=?1",
                    [hash],
                )?;
            }
            Ok(user)
        })
    }
    pub fn keys(&self, user: i64) -> Result<Value> {
        let db = self.db.lock().unwrap();
        let mut q = db.prepare("SELECT id,name FROM api_keys WHERE owner_id=?1 AND revoked=0")?;
        let rows = q
            .query_map([user], |r| {
                Ok(json!({"id":r.get::<_,i64>(0)?,"name":r.get::<_,Option<String>>(1)?}))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!(rows))
    }
    pub fn revoke(&self, user: i64, key: i64) -> Result<bool> {
        self.transaction(|tx| {
            Ok(tx.execute(
                "UPDATE api_keys SET revoked=1 WHERE owner_id=?1 AND id=?2",
                params![user, key],
            )? == 1)
        })
    }
    pub fn system_count(&self) -> Result<i64> {
        let db = self.db.lock().unwrap();
        Ok(db.query_row(
            "SELECT count(*) FROM codes WHERE used=0 AND owner_id=0",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn codes(&self, user: i64) -> Result<Vec<String>> {
        let db = self.db.lock().unwrap();
        let mut q =
            db.prepare("SELECT code FROM codes WHERE owner_id=?1 AND used=0 ORDER BY id")?;
        Ok(q.query_map([user], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn generate(&self, count: i64) -> Result<()> {
        ensure!((0..=100000).contains(&count), "Invalid count");
        self.transaction(|tx| {
            for _ in 0..count {
                tx.execute(
                    "INSERT INTO codes(code,owner_id,used) VALUES(?1,0,0)",
                    [random_hex(8)],
                )?;
            }
            Ok(())
        })
    }
    pub fn replenish(&self, capacity: i64) -> Result<()> {
        ensure!((1..=100000).contains(&capacity), "Invalid capacity");
        self.transaction(|tx| {
            let current: i64 =
                tx.query_row("SELECT count(*) FROM codes WHERE used=0", [], |r| r.get(0))?;
            for _ in current..capacity {
                tx.execute(
                    "INSERT INTO codes(code,owner_id,used) VALUES(?1,0,0)",
                    [random_hex(8)],
                )?;
            }
            Ok(())
        })
    }
    pub fn reset_codes(&self) -> Result<usize> {
        self.transaction(|tx| Ok(tx.execute("DELETE FROM codes WHERE owner_id=0 AND used=0", [])?))
    }
    pub fn redeem(&self, text: &str) -> Result<bool> {
        self.transaction(|tx| {
            let codes = {
                let mut q = tx.prepare("SELECT id,code FROM codes WHERE used=0")?;
                q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (id, code) in codes {
                if !code.is_empty() && text.contains(&code) {
                    return Ok(tx.execute(
                        "UPDATE codes SET used=1,used_at=CURRENT_TIMESTAMP WHERE id=?1 AND used=0",
                        [id],
                    )? == 1);
                }
            }
            Ok(false)
        })
    }
    pub fn price(config: &Value, available: i64) -> Result<f64> {
        ensure!(available > 0, "Not enough codes");
        let min = config["Pmin"].as_f64().context("Pmin must be numeric")?;
        let a = config["constA"]
            .as_f64()
            .context("constA must be numeric")?;
        let capacity = id(&config["count"]);
        let price = min * (1.0 + a * ((capacity - available) as f64 / available as f64));
        ensure!(
            price.is_finite() && price >= 0.0,
            "Invalid market configuration"
        );
        Ok(price)
    }
    pub fn trade(&self, user: i64, count: i64, buy: bool, config: &Value) -> Result<Value> {
        ensure!((1..=100000).contains(&count), "Invalid count");
        self.transaction(|tx|{
        let q:i64=tx.query_row("SELECT count(*) FROM codes WHERE used=0 AND owner_id=0",[],|r|r.get(0))?;let balance=Self::balance_tx(tx,user)?;
        let owner=if buy {0}else{user};let available:i64=tx.query_row("SELECT count(*) FROM codes WHERE used=0 AND owner_id=?1",[owner],|r|r.get(0))?;ensure!(available>=count,"Not enough codes");
        let mut total=0i64;let coeff=config["coeff"].as_f64().unwrap_or(0.9);ensure!((0.0..=1.0).contains(&coeff),"Invalid coeff");
        for n in 0..count {let price=Self::price(config,if buy{q-n}else{q+n+1})?*if buy{1.0}else{coeff};ensure!(price<=i64::MAX as f64,"Price overflow");total=total.checked_add(price as i64).context("Price overflow")?;}
        if total>0 {Self::transfer_tx(tx,if buy{user}else{0},if buy{0}else{user},total,if buy{"purchase of anonymous codes"}else{"sale of anonymous code"})?;}
        let destination=if buy {user}else{0};tx.execute("UPDATE codes SET owner_id=?2 WHERE id IN (SELECT id FROM codes WHERE used=0 AND owner_id=?1 ORDER BY id LIMIT ?3)",params![owner,destination,count])?;
        tx.execute("INSERT INTO market_operations(type,amount,total) VALUES(?1,?2,?3)",params![if buy{"buy"}else{"sell"},count,total])?;
        let remaining=if buy{q-count}else{q+count};let price=if remaining>0 {Self::price(config,remaining)?}else{0.0};tx.execute("INSERT INTO market_history(price,available_codes) VALUES(?1,?2)",params![price,remaining])?;
        Ok(json!({"status":"ok","user_id":user,"count":count,"price":total,"balance":if buy{balance-total}else{balance+total}}))
    })
    }
    pub fn market(&self, config: &Value) -> Result<Value> {
        self.transaction(|tx|{let q:i64=tx.query_row("SELECT count(*) FROM codes WHERE used=0 AND owner_id=0",[],|r|r.get(0))?;let unused:i64=tx.query_row("SELECT count(*) FROM codes WHERE used=0",[],|r|r.get(0))?;let price=if q>0{Self::price(config,q)?}else{0.0};let old:Option<f64>=tx.query_row("SELECT price FROM market_state WHERE id=1",[],|r|r.get(0)).optional()?;
        tx.execute("INSERT INTO market_state(id,available_codes,price) VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET available_codes=?1,price=?2,updated_at=CURRENT_TIMESTAMP",params![q,price])?;
        Ok(json!({"price":price as i64,"price2":(price*config["coeff"].as_f64().unwrap_or(0.9)) as i64,"availableCodes":q,"unusedCodes":unused,"capacity":config["count"],"priceChange":old.filter(|p|*p>0.0).map(|p|(price-p)/p*100.0).unwrap_or(0.0),"status":if q>0{"ACTIVE"}else{"SOLD_OUT"}}))})
    }
    pub fn history(&self, hours: i64) -> Result<Value> {
        let db = self.db.lock().unwrap();
        let mut q=db.prepare("SELECT created_at,price FROM market_history WHERE created_at>=datetime('now',?1) ORDER BY id DESC LIMIT 500")?;
        let rows=q.query_map([format!("-{hours} hours")],|r|{let timestamp:String=r.get(0)?;let price:f64=r.get(1)?;Ok(json!({"timestamp":timestamp,"price":price,"open":price,"high":price,"low":price,"close":price}))})?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!(rows.into_iter().rev().collect::<Vec<_>>()))
    }
    pub fn operations(&self) -> Result<Value> {
        let db = self.db.lock().unwrap();
        let mut q = db.prepare(
            "SELECT type,amount,total,created_at FROM market_operations ORDER BY id DESC LIMIT 20",
        )?;
        let rows=q.query_map([],|r|Ok(json!({"type":r.get::<_,String>(0)?,"amount":r.get::<_,i64>(1)?,"total":r.get::<_,f64>(2)?,"timestamp":r.get::<_,String>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!(rows))
    }
    pub fn bonus(&self, user: i64) -> Result<Value> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let u1 = rng.gen_range(f64::EPSILON..1.0);
        let u2 = rng.gen_range(0.0..1.0);
        let coins = (500.0 + 165.0 * (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos())
            .round()
            .clamp(0.0, 1000.0) as i64;
        self.transaction(|tx|{Self::add(tx,user,None)?;let elapsed:i64=tx.query_row("SELECT coalesce(cast(strftime('%s','now') AS INTEGER)-cast(strftime('%s',last_salary) AS INTEGER),43200) FROM users WHERE user_id=?1",[user],|r|r.get(0))?;
            if elapsed<43200 {return Ok(json!({"error":"Bonus not available","remaining":{"hours":(43200-elapsed)/3600,"minutes":((43200-elapsed)%3600)/60}}))}
            if coins>0{Self::transfer_tx(tx,0,user,coins,"daily_gift")?;}tx.execute("UPDATE users SET last_salary=CURRENT_TIMESTAMP WHERE user_id=?1",[user])?;Ok(json!({"status":"ok","user_id":user,"amount":coins,"balance":Self::balance_tx(tx,user)?}))})
    }
    pub fn gift_codes(&self, from: i64, to: i64, count: i64) -> Result<()> {
        ensure!(count > 0 && from != to, "Invalid count or recipient");
        self.transaction(|tx|{let q:i64=tx.query_row("SELECT count(*) FROM codes WHERE used=0 AND owner_id=?1",[from],|r|r.get(0))?;ensure!(q>=count,"Not enough codes");tx.execute("UPDATE codes SET owner_id=?2 WHERE id IN (SELECT id FROM codes WHERE used=0 AND owner_id=?1 ORDER BY id LIMIT ?3)",params![from,to,count])?;Ok(())})
    }
}
