use std::path::PathBuf;

use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use r2d2_sqlite::rusqlite::{params, OptionalExtension};

use crate::errors::{AppError, AppResult};
use crate::models::{AuditEntry, Config, Price, SymbolMap};

#[derive(Clone)]
pub struct DbState {
	pool: Pool<SqliteConnectionManager>,
}

impl DbState {
	pub fn initialize() -> AppResult<Self> {
		let db_path = std::env::var("ORACLE_DB_PATH").unwrap_or_else(|_| "./oracle.sqlite".into());
		let path = PathBuf::from(db_path);
		let manager = SqliteConnectionManager::file(path).with_init(|c| {
			c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")
		});
		let pool = Pool::builder().max_size(8).build(manager).map_err(|e| AppError::Anyhow(e.into()))?;
		let state = Self { pool };
		state.migrate()?;
		Ok(state)
	}

	fn conn(&self) -> AppResult<PooledConnection<SqliteConnectionManager>> {
		self.pool.get().map_err(|e| AppError::Anyhow(e.into()))
	}

	fn migrate(&self) -> AppResult<()> {
		let conn = self.conn()?;
		conn.execute_batch(
			"CREATE TABLE IF NOT EXISTS prices (
				mint TEXT PRIMARY KEY,
				symbol TEXT,
				usd_mantissa TEXT NOT NULL,
				usd_scale INTEGER NOT NULL,
				updated_at TEXT NOT NULL,
				updated_by TEXT NOT NULL,
				decimals INTEGER
			);
			CREATE TABLE IF NOT EXISTS symbols (
				symbol TEXT PRIMARY KEY,
				mint TEXT NOT NULL
			);
			CREATE TABLE IF NOT EXISTS config (
				id INTEGER PRIMARY KEY CHECK (id = 1),
				network TEXT NOT NULL,
				version TEXT NOT NULL,
				fee_bps_default INTEGER NOT NULL,
				zera_mint TEXT NOT NULL,
				supported_mints TEXT NOT NULL -- JSON array
			);
			INSERT OR IGNORE INTO config (id, network, version, fee_bps_default, zera_mint, supported_mints)
			VALUES (1, 'devnet', 'v0.1', 100, '', '[]');
			CREATE TABLE IF NOT EXISTS audit (
				id TEXT PRIMARY KEY,
				ts TEXT NOT NULL,
				actor TEXT NOT NULL,
				action TEXT NOT NULL,
				target TEXT NOT NULL,
				before TEXT,
				after TEXT
			);
			CREATE TABLE IF NOT EXISTS http_cache (
				cache_key TEXT PRIMARY KEY,
				status INTEGER NOT NULL,
				body TEXT NOT NULL,
				stored_at INTEGER NOT NULL,
				expires_at INTEGER NOT NULL,
				popularity REAL NOT NULL DEFAULT 0.0,
				last_accessed INTEGER NOT NULL
			);
			CREATE INDEX IF NOT EXISTS idx_http_cache_expires ON http_cache (expires_at);
			CREATE INDEX IF NOT EXISTS idx_http_cache_popularity ON http_cache (popularity DESC);",
		)?;
		Ok(())
	}

	pub fn get_config(&self) -> AppResult<Config> {
		let conn = self.conn()?;
		let row = conn.query_row(
			"SELECT network, version, fee_bps_default, zera_mint, supported_mints FROM config WHERE id = 1",
			[],
			|r| {
				Ok(Config {
					network: r.get(0)?,
					version: r.get(1)?,
					fee_bps_default: r.get::<_, i64>(2)? as u16,
					zera_mint: r.get(3)?,
					supported_mints: serde_json::from_str::<Vec<String>>(&r.get::<_, String>(4)?).unwrap_or_default(),
				})
			},
		)?;
		Ok(row)
	}

	pub fn update_config(&self, patch: serde_json::Value, actor: &str) -> AppResult<Config> {
		let before = serde_json::to_value(self.get_config()?)?;
		let mut cfg: Config = serde_json::from_value(before.clone())?;

		if let Some(v) = patch.get("network").and_then(|v| v.as_str()) { cfg.network = v.to_string(); }
		if let Some(v) = patch.get("version").and_then(|v| v.as_str()) { cfg.version = v.to_string(); }
		if let Some(v) = patch.get("fee_bps_default").and_then(|v| v.as_u64()) { cfg.fee_bps_default = v as u16; }
		if let Some(v) = patch.get("zera_mint").and_then(|v| v.as_str()) { cfg.zera_mint = v.to_string(); }
		if let Some(v) = patch.get("supported_mints").and_then(|v| v.as_array()) {
			cfg.supported_mints = v.iter().filter_map(|x| x.as_str()).map(|s| s.to_string()).collect();
		}

		let conn = self.conn()?;
		conn.execute(
			"UPDATE config SET network = ?, version = ?, fee_bps_default = ?, zera_mint = ?, supported_mints = ? WHERE id = 1",
			params![cfg.network, cfg.version, cfg.fee_bps_default as i64, cfg.zera_mint, serde_json::to_string(&cfg.supported_mints)?],
		)?;

		self.insert_audit("PATCH_CONFIG", actor, "config", Some(before), Some(serde_json::to_value(&cfg)?))?;
		Ok(cfg)
	}

	pub fn list_prices(&self) -> AppResult<Vec<Price>> {
		let conn = self.conn()?;
		let mut stmt = conn.prepare("SELECT mint, symbol, usd_mantissa, usd_scale, updated_at, updated_by, decimals FROM prices ORDER BY mint")?;
		let rows = stmt.query_map([], |r| {
			Ok(Price {
				mint: r.get(0)?,
				symbol: r.get(1)?,
				usd_mantissa: r.get(2)?,
				usd_scale: r.get::<_, i64>(3)? as u32,
				updated_at: r.get(4)?,
				updated_by: r.get(5)?,
				decimals: r.get(6)?,
			})
		})?;
		Ok(rows.filter_map(Result::ok).collect())
	}

	pub fn get_price(&self, mint: &str) -> AppResult<Price> {
		let conn = self.conn()?;
		let row = conn
			.query_row(
				"SELECT mint, symbol, usd_mantissa, usd_scale, updated_at, updated_by, decimals FROM prices WHERE mint = ?",
				params![mint],
				|r| {
					Ok(Price {
						mint: r.get(0)?,
						symbol: r.get(1)?,
						usd_mantissa: r.get(2)?,
						usd_scale: r.get::<_, i64>(3)? as u32,
						updated_at: r.get(4)?,
						updated_by: r.get(5)?,
						decimals: r.get(6)?,
					})
				},
			)
			.optional()?;
		row.ok_or(AppError::NotFound)
	}

	pub fn upsert_price(&self, price: &Price, actor: &str) -> AppResult<Price> {
		let conn = self.conn()?;
		let before = self
			.get_price(&price.mint)
			.ok()
			.and_then(|p| serde_json::to_value(p).ok());

		conn.execute(
			"INSERT INTO prices (mint, symbol, usd_mantissa, usd_scale, updated_at, updated_by, decimals) VALUES (?, ?, ?, ?, ?, ?, ?)
			ON CONFLICT(mint) DO UPDATE SET symbol = excluded.symbol, usd_mantissa = excluded.usd_mantissa, usd_scale = excluded.usd_scale, updated_at = excluded.updated_at, updated_by = excluded.updated_by, decimals = excluded.decimals",
			params![
				price.mint,
				price.symbol.clone(),
				price.usd_mantissa,
				price.usd_scale as i64,
				price.updated_at,
				price.updated_by,
				price.decimals.map(|d| d as i64)
			],
		)?;

		self.insert_audit("UPSERT_PRICE", actor, &price.mint, before, Some(serde_json::to_value(price)?))?;
		self.get_price(&price.mint)
	}

	pub fn patch_price(&self, mint: &str, patch: serde_json::Value, actor: &str) -> AppResult<Price> {
		let before = self.get_price(mint)?;
		let mut price = before.clone();
		if let Some(v) = patch.get("symbol").and_then(|v| v.as_str()) { price.symbol = Some(v.to_string()); }
		if let Some(v) = patch.get("usd_mantissa").and_then(|v| v.as_str()) { price.usd_mantissa = v.to_string(); }
		if let Some(v) = patch.get("usd_scale").and_then(|v| v.as_u64()) { price.usd_scale = v as u32; }
		if let Some(v) = patch.get("decimals").and_then(|v| v.as_u64()) { price.decimals = Some(v as u8); }
		price.updated_at = Price::now_iso();
		price.updated_by = format!("admin:{}", actor);

		let conn = self.conn()?;
		conn.execute(
			"UPDATE prices SET symbol = ?, usd_mantissa = ?, usd_scale = ?, updated_at = ?, updated_by = ?, decimals = ? WHERE mint = ?",
			params![
				price.symbol.clone(), price.usd_mantissa, price.usd_scale as i64, price.updated_at, price.updated_by, price.decimals.map(|d| d as i64), price.mint
			],
		)?;
		self.insert_audit("UPSERT_PRICE", actor, mint, Some(serde_json::to_value(before)?), Some(serde_json::to_value(&price)?))?;
		Ok(price)
	}

	pub fn delete_price(&self, mint: &str, actor: &str) -> AppResult<()> {
		let before = self.get_price(mint).ok().and_then(|p| serde_json::to_value(p).ok());
		let conn = self.conn()?;
		let n = conn.execute("DELETE FROM prices WHERE mint = ?", params![mint])?;
		if n == 0 { return Err(AppError::NotFound); }
		self.insert_audit("DELETE_PRICE", actor, mint, before, None)?;
		Ok(())
	}

	pub fn list_symbols(&self) -> AppResult<Vec<SymbolMap>> {
		let conn = self.conn()?;
		let mut stmt = conn.prepare("SELECT symbol, mint FROM symbols ORDER BY symbol")?;
		let rows = stmt.query_map([], |r| Ok(SymbolMap { symbol: r.get(0)?, mint: r.get(1)? }))?;
		Ok(rows.filter_map(Result::ok).collect())
	}

	pub fn upsert_symbol(&self, symbol: &str, mint: &str) -> AppResult<()> {
		let conn = self.conn()?;
		conn.execute(
			"INSERT INTO symbols (symbol, mint) VALUES (?, ?) ON CONFLICT(symbol) DO UPDATE SET mint = excluded.mint",
			params![symbol, mint],
		)?;
		Ok(())
	}

	pub fn insert_audit(
		&self,
		action: &str,
		actor: &str,
		target: &str,
		before: Option<serde_json::Value>,
		after: Option<serde_json::Value>,
	) -> AppResult<()> {
		let entry = AuditEntry::new(
			action,
			actor,
			target,
			before,
			after,
		);
		let conn = self.conn()?;
		conn.execute(
			"INSERT INTO audit (id, ts, actor, action, target, before, after) VALUES (?, ?, ?, ?, ?, ?, ?)",
			params![
				entry.id,
				entry.ts,
				entry.actor,
				entry.action,
				entry.target,
				entry.before.map(|v| v.to_string()),
				entry.after.map(|v| v.to_string()),
			],
		)?;
		Ok(())
	}

	pub fn list_audit(&self, limit: usize, cursor: Option<String>) -> AppResult<(Vec<AuditEntry>, Option<String>)> {
		let conn = self.conn()?;
		let mut stmt = if cursor.is_some() {
			conn.prepare("SELECT id, ts, actor, action, target, before, after FROM audit WHERE id < ? ORDER BY id DESC LIMIT ?")?
		} else {
			conn.prepare("SELECT id, ts, actor, action, target, before, after FROM audit ORDER BY id DESC LIMIT ?")?
		};
		let rows = if let Some(c) = cursor {
			stmt.query_map(params![c, limit as i64], map_audit_row)?
		} else {
			stmt.query_map(params![limit as i64], map_audit_row)?
		};
		let mut entries: Vec<AuditEntry> = Vec::new();
		for res in rows {
			if let Ok(e) = res { entries.push(e); }
		}
		let next_cursor = entries.last().map(|e| e.id.clone());
		Ok((entries, next_cursor))
	}
}

// ================= L2 HTTP cache helpers =================
impl DbState {
	pub fn http_cache_get(&self, cache_key: &str, now_epoch: i64) -> AppResult<Option<(u16, String, i64)>> {
		let conn = self.conn()?;
		let mut stmt = conn.prepare("SELECT status, body, expires_at FROM http_cache WHERE cache_key = ?")?;
		let row = stmt.query_row(params![cache_key], |r| {
			Ok((
				r.get::<_, i64>(0)? as u16,
				r.get::<_, String>(1)?,
				r.get::<_, i64>(2)?,
			))
		}).optional()?;
		if row.is_some() {
			let _ = conn.execute("UPDATE http_cache SET last_accessed = ?, popularity = popularity * 0.95 + 1.0 WHERE cache_key = ?", params![now_epoch, cache_key]);
		}
		Ok(row)
	}

	pub fn http_cache_put(&self, cache_key: &str, status: u16, body: &str, ttl_secs: i64, now_epoch: i64) -> AppResult<()> {
		let conn = self.conn()?;
		let expires_at = now_epoch + ttl_secs;
		conn.execute(
			"INSERT INTO http_cache (cache_key, status, body, stored_at, expires_at, popularity, last_accessed) VALUES (?, ?, ?, ?, ?, 1.0, ?)
			ON CONFLICT(cache_key) DO UPDATE SET status = excluded.status, body = excluded.body, stored_at = excluded.stored_at, expires_at = excluded.expires_at",
			params![cache_key, status as i64, body, now_epoch, expires_at, now_epoch],
		)?;
		Ok(())
	}

	pub fn http_cache_mark_access(&self, cache_key: &str, now_epoch: i64) -> AppResult<()> {
		let conn = self.conn()?;
		let _ = conn.execute("UPDATE http_cache SET last_accessed = ?, popularity = popularity * 0.95 + 1.0 WHERE cache_key = ?", params![now_epoch, cache_key]);
		Ok(())
	}

	pub fn http_cache_list_hot_keys(&self, limit: usize) -> AppResult<Vec<String>> {
		let conn = self.conn()?;
		let mut stmt = conn.prepare("SELECT cache_key FROM http_cache ORDER BY popularity DESC LIMIT ?")?;
		let rows = stmt.query_map(params![limit as i64], |r| Ok(r.get::<_, String>(0)?))?;
		Ok(rows.filter_map(Result::ok).collect())
	}

	pub fn http_cache_cleanup_expired(&self, now_epoch: i64, max_rows: usize) -> AppResult<usize> {
		let conn = self.conn()?;
		let n = conn.execute("DELETE FROM http_cache WHERE expires_at < ? LIMIT ?", params![now_epoch, max_rows as i64])?;
		Ok(n)
	}
}

fn row_to_audit(r: &r2d2_sqlite::rusqlite::Row<'_>) -> AuditEntry {
	AuditEntry {
		id: r.get(0).unwrap_or_default(),
		ts: r.get(1).unwrap_or_default(),
		actor: r.get(2).unwrap_or_default(),
		action: r.get(3).unwrap_or_default(),
		target: r.get(4).unwrap_or_default(),
		before: r.get::<_, Option<String>>(5).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()),
		after: r.get::<_, Option<String>>(6).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()),
	}
}

fn map_audit_row(r: &r2d2_sqlite::rusqlite::Row<'_>) -> Result<AuditEntry, r2d2_sqlite::rusqlite::Error> {
	Ok(row_to_audit(r))
} 