use anyhow::Result;
use chrono::Utc;
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use sha2::{Sha256, Digest};

#[derive(Clone)]
pub struct StakingState {
    pool: Arc<Pool<SqliteConnectionManager>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProviderRecord {
    pub provider_id: String,
    pub stake: f64,
    pub rewards: f64,
    pub slashed: f64,
    pub updated_at: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EventRecord {
    pub event_type: String,
    pub amount: f64,
    pub memo: Option<String>,
    pub ts: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CommitRecord {
    pub task_id: String,
    pub provider_id: String,
    pub commitment_hash: String,
    pub revealed_result: Option<String>,
    pub salt: Option<String>,
    pub status: String, // 'committed', 'revealed', 'disputed'
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DisputeRecord {
    pub id: i64,
    pub task_id: String,
    pub accuser_id: String,
    pub defendant_id: String,
    pub evidence: Option<String>,
    pub status: String, // 'pending', 'resolved_guilty', 'resolved_innocent'
    pub created_at: String,
}

impl StakingState {
    pub fn new(base_dir: &Path) -> Result<Self> {
        let db_path = base_dir.join("staking.db");
        let manager = SqliteConnectionManager::file(db_path);
        let pool = Pool::new(manager)?;
        {
            let conn = pool.get()?;
            init_schema(&conn)?;
        }
        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    fn conn(&self) -> Result<PooledConnection<SqliteConnectionManager>> {
        Ok(self.pool.get()?)
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn init_schema(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS providers (
            provider_id TEXT PRIMARY KEY,
            stake REAL NOT NULL DEFAULT 0,
            rewards REAL NOT NULL DEFAULT 0,
            slashed REAL NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            amount REAL NOT NULL,
            memo TEXT,
            ts TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS commits (
            task_id TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            commitment_hash TEXT NOT NULL,
            revealed_result TEXT,
            salt TEXT,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (task_id, provider_id)
        );
        CREATE TABLE IF NOT EXISTS disputes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id TEXT NOT NULL,
            accuser_id TEXT NOT NULL,
            defendant_id TEXT NOT NULL,
            evidence TEXT,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
    "#,
    )?;
    Ok(())
}

// core DB helpers (not public)
fn log_event_db(
    conn: &rusqlite::Connection,
    pid: &str,
    event_type: &str,
    amount: f64,
    memo: Option<String>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO events (provider_id, event_type, amount, memo, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![pid, event_type, amount, memo, now()],
    )?;
    Ok(())
}

fn load_provider_db(conn: &rusqlite::Connection, pid: &str) -> Result<Option<ProviderRecord>> {
    let mut stmt = conn.prepare(
        "SELECT provider_id, stake, rewards, slashed, updated_at FROM providers WHERE provider_id = ?1",
    )?;
    let rec = stmt
        .query_map([pid], |row| {
            Ok(ProviderRecord {
                provider_id: row.get(0)?,
                stake: row.get(1)?,
                rewards: row.get(2)?,
                slashed: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?
        .next()
        .transpose()?;
    Ok(rec)
}

fn upsert_provider_db(conn: &rusqlite::Connection, pid: &str, stake: f64) -> Result<ProviderRecord> {
    let ts = now();
    conn.execute(
        "INSERT INTO providers (provider_id, stake, rewards, slashed, updated_at)
         VALUES (?1, ?2, 0, 0, ?3)
         ON CONFLICT(provider_id) DO UPDATE SET updated_at=excluded.updated_at",
        params![pid, stake, ts],
    )?;
    log_event_db(conn, pid, "register", stake, None)?;
    Ok(ProviderRecord {
        provider_id: pid.to_string(),
        stake,
        rewards: 0.0,
        slashed: 0.0,
        updated_at: ts,
    })
}

// Public API
impl StakingState {
    pub fn register(&self, pid: &str, stake: f64) -> Result<ProviderRecord> {
        let conn = self.conn()?;
        let rec = upsert_provider_db(&conn, pid, stake)?;
        Ok(rec)
    }

    pub fn get_provider(&self, pid: &str) -> Result<Option<ProviderRecord>> {
        let conn = self.conn()?;
        load_provider_db(&conn, pid)
    }

    pub fn stake(&self, pid: &str, amount: f64) -> Result<ProviderRecord> {
        if amount <= 0.0 {
            anyhow::bail!("amount must be positive");
        }
        let conn = self.conn()?;
        let mut rec = match load_provider_db(&conn, pid)? {
            Some(r) => r,
            None => anyhow::bail!("provider not registered"),
        };
        rec.stake += amount;
        conn.execute(
            "UPDATE providers SET stake=?1, rewards=?2, slashed=?3, updated_at=?4 WHERE provider_id=?5",
            params![rec.stake, rec.rewards, rec.slashed, now(), pid],
        )?;
        log_event_db(&conn, pid, "stake", amount, None)?;
        Ok(rec)
    }

    pub fn reward(&self, pid: &str, amount: f64) -> Result<ProviderRecord> {
        if amount <= 0.0 {
            anyhow::bail!("amount must be positive");
        }
        let conn = self.conn()?;
        let mut rec = match load_provider_db(&conn, pid)? {
            Some(r) => r,
            None => anyhow::bail!("provider not registered"),
        };
        rec.rewards += amount;
        conn.execute(
            "UPDATE providers SET stake=?1, rewards=?2, slashed=?3, updated_at=?4 WHERE provider_id=?5",
            params![rec.stake, rec.rewards, rec.slashed, now(), pid],
        )?;
        log_event_db(&conn, pid, "reward", amount, None)?;
        Ok(rec)
    }

    pub fn slash(&self, pid: &str, amount: f64, reason: Option<String>) -> Result<ProviderRecord> {
        if amount <= 0.0 {
            anyhow::bail!("amount must be positive");
        }
        let conn = self.conn()?;
        let mut rec = match load_provider_db(&conn, pid)? {
            Some(r) => r,
            None => anyhow::bail!("provider not registered"),
        };
        let mut remaining = amount;
        if rec.stake >= remaining {
            rec.stake -= remaining;
            remaining = 0.0;
        } else {
            remaining -= rec.stake;
            rec.stake = 0.0;
        }
        if remaining > 0.0 {
            if rec.rewards >= remaining {
                rec.rewards -= remaining;
                remaining = 0.0;
            }
        }
        // NOTE: We allow slashing more than available funds (debt?) or just cap at 0?
        // For now, let's just cap at 0 and record the slash amount.
        // if remaining > 0.0 { anyhow::bail!("insufficient funds to slash"); }
        
        rec.slashed += amount;
        conn.execute(
            "UPDATE providers SET stake=?1, rewards=?2, slashed=?3, updated_at=?4 WHERE provider_id=?5",
            params![rec.stake, rec.rewards, rec.slashed, now(), pid],
        )?;
        log_event_db(&conn, pid, "slash", amount, reason)?;
        Ok(rec)
    }

    pub fn withdraw(&self, pid: &str, amount: f64) -> Result<ProviderRecord> {
        if amount <= 0.0 {
            anyhow::bail!("amount must be positive");
        }
        let conn = self.conn()?;
        let mut rec = match load_provider_db(&conn, pid)? {
            Some(r) => r,
            None => anyhow::bail!("provider not registered"),
        };
        if rec.rewards < amount {
            anyhow::bail!("insufficient rewards to withdraw");
        }
        rec.rewards -= amount;
        conn.execute(
            "UPDATE providers SET stake=?1, rewards=?2, slashed=?3, updated_at=?4 WHERE provider_id=?5",
            params![rec.stake, rec.rewards, rec.slashed, now(), pid],
        )?;
        log_event_db(&conn, pid, "withdraw", amount, None)?;
        Ok(rec)
    }

    pub fn get_events(&self, pid: &str) -> Result<Vec<EventRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT event_type, amount, memo, ts FROM events WHERE provider_id=?1 ORDER BY id DESC LIMIT 100",
        )?;
        let rows = stmt
            .query_map([pid], |row| {
                Ok(EventRecord {
                    event_type: row.get(0)?,
                    amount: row.get(1)?,
                    memo: row.get(2)?,
                    ts: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // --- Consensus / Commit-Reveal Logic ---

    pub fn commit(&self, task_id: &str, pid: &str, hash: &str) -> Result<()> {
        let conn = self.conn()?;
        // Check if provider exists
        if load_provider_db(&conn, pid)?.is_none() {
            anyhow::bail!("Provider not registered");
        }
        
        let ts = now();
        conn.execute(
            "INSERT INTO commits (task_id, provider_id, commitment_hash, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'committed', ?4, ?4)",
            params![task_id, pid, hash, ts],
        )?;
        Ok(())
    }

    pub fn reveal(&self, task_id: &str, pid: &str, result: &str, salt: &str) -> Result<bool> {
        let conn = self.conn()?;
        
        // 1. Fetch commitment
        let mut stmt = conn.prepare(
            "SELECT commitment_hash FROM commits WHERE task_id=?1 AND provider_id=?2"
        )?;
        let hash: String = match stmt.query_row(params![task_id, pid], |row| row.get(0)) {
            Ok(h) => h,
            Err(_) => anyhow::bail!("Commitment not found"),
        };

        // 2. Verify Hash
        let mut hasher = Sha256::new();
        hasher.update(format!("{}{}", result, salt));
        let computed_hash = format!("{:x}", hasher.finalize());

        if computed_hash != hash {
            return Ok(false);
        }

        // 3. Update state
        let ts = now();
        conn.execute(
            "UPDATE commits SET revealed_result=?1, salt=?2, status='revealed', updated_at=?3
             WHERE task_id=?4 AND provider_id=?5",
            params![result, salt, ts, task_id, pid],
        )?;

        Ok(true)
    }

    pub fn challenge(&self, task_id: &str, accuser: &str, defendant: &str, evidence: Option<String>) -> Result<i64> {
        let conn = self.conn()?;
        
        // Check if defendant committed
        let mut stmt = conn.prepare("SELECT count(*) FROM commits WHERE task_id=?1 AND provider_id=?2")?;
        if stmt.query_row(params![task_id, defendant], |row| row.get::<_, i64>(0))? == 0 {
             anyhow::bail!("Defendant has no commitment for this task");
        }

        let ts = now();
        conn.execute(
            "INSERT INTO disputes (task_id, accuser_id, defendant_id, evidence, status, created_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![task_id, accuser, defendant, evidence, ts],
        )?;
        
        let dispute_id = conn.last_insert_rowid();
        
        // Mark commit as disputed
        conn.execute(
            "UPDATE commits SET status='disputed', updated_at=?1 WHERE task_id=?2 AND provider_id=?3",
            params![ts, task_id, defendant],
        )?;

        Ok(dispute_id)
    }

    pub fn resolve_dispute(&self, dispute_id: i64, guilty: bool) -> Result<()> {
        let conn = self.conn()?;
        
        // Fetch dispute info
        let mut stmt = conn.prepare("SELECT task_id, accuser_id, defendant_id FROM disputes WHERE id=?1")?;
        let (task_id, accuser, defendant): (String, String, String) = stmt.query_row(params![dispute_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;

        let status = if guilty { "resolved_guilty" } else { "resolved_innocent" };
        
        conn.execute(
            "UPDATE disputes SET status=?1 WHERE id=?2",
            params![status, dispute_id],
        )?;

        if guilty {
            // Slash defendant
            // 100 GLM penalty for example
            // We call self.slash logic but we need to do it within this scope. 
            // Re-implementing simplified slash here to reuse conn is better, or just use the public API logic inside a transaction.
            // For simplicity, I'll just reuse the logic inline.
            
            // NOTE: In a real DB, we should use a transaction here.
            
            // Slash 100
            let slash_amount = 100.0;
            self.slash(defendant.as_str(), slash_amount, Some(format!("Dispute #{}", dispute_id)))?;
            
            // Reward accuser (50% of slash)
            self.reward(accuser.as_str(), slash_amount * 0.5)?;
        }

        Ok(())
    }
}
