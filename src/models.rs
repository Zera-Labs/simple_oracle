use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Price {
	pub mint: String,
	#[serde(skip_serializing_if = "Option::is_none")] 
	pub symbol: Option<String>,
	pub usd_mantissa: String,
	pub usd_scale: u32,
	pub updated_at: String,
	pub updated_by: String,
	#[serde(skip_serializing_if = "Option::is_none")] 
	pub decimals: Option<u8>,
}

impl Price {
	pub fn now_iso() -> String {
		OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_else(|_| "".into())
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolMap {
	pub symbol: String,
	pub mint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
	pub network: String,
	pub version: String,
	pub fee_bps_default: u16,
	pub zera_mint: String,
	pub supported_mints: Vec<String>,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			network: std::env::var("ORACLE_NETWORK").unwrap_or_else(|_| "devnet".into()),
			version: "v0.1".into(),
			fee_bps_default: std::env::var("DEFAULT_FEE_BPS").ok().and_then(|v| v.parse().ok()).unwrap_or(100),
			zera_mint: std::env::var("ZERA_MINT").unwrap_or_default(),
			supported_mints: std::env::var("SUPPORTED_MINTS").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default(),
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
	pub id: String,
	pub ts: String,
	pub actor: String,
	pub action: String,
	pub target: String,
	#[serde(skip_serializing_if = "Option::is_none")] 
	pub before: Option<serde_json::Value>,
	#[serde(skip_serializing_if = "Option::is_none")] 
	pub after: Option<serde_json::Value>,
}

impl AuditEntry {
	pub fn new(action: &str, actor: &str, target: &str, before: Option<serde_json::Value>, after: Option<serde_json::Value>) -> Self {
		let ts = OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default();
		Self {
			id: Uuid::new_v4().to_string(),
			ts,
			actor: actor.to_string(),
			action: action.to_string(),
			target: target.to_string(),
			before,
			after,
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedAuditResponse {
	pub entries: Vec<AuditEntry>,
	#[serde(skip_serializing_if = "Option::is_none")] 
	pub next_cursor: Option<String>,
} 