use dashmap::DashMap;
use rocket::http::Status;
use std::time::{Duration, Instant};

use crate::errors::{AppError, AppResult};
use crate::realtime::Broadcaster;

#[derive(Clone)]
pub struct HeliusPriceService {
	client: reqwest::Client,
	api_url: String,
	ttl: Duration,
	cache: DashMap<String, PriceCache>,
}

#[derive(Clone)]
struct PriceCache {
	usd: f64,
	stored_at: Instant,
}

impl HeliusPriceService {
	pub fn from_env() -> Self {
		let api_url = match std::env::var("HELIUS_RPC_URL") {
			Ok(url) => url,
			Err(_) => {
				let key = std::env::var("HELIUS_API_KEY").unwrap_or_default();
				if key.is_empty() { String::new() } else { format!("https://mainnet.helius-rpc.com/?api-key={}", key) }
			}
		};
		let ttl_secs = std::env::var("HELIUS_TTL_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(5);
		let client = reqwest::Client::builder()
			.user_agent("zera-oracle-helius/1.0")
			.timeout(Duration::from_millis(5_000))
			.build()
			.expect("failed to build reqwest client");
		Self { client, api_url, ttl: Duration::from_secs(ttl_secs), cache: DashMap::new() }
	}

	pub async fn get_cached_price(&self, mint: &str) -> AppResult<(Status, String)> {
		let now = Instant::now();
		if let Some(entry) = self.cache.get(mint) {
			if now.duration_since(entry.stored_at) < self.ttl {
				let body = serde_json::json!({
					"mint": mint,
					"usd": entry.usd,
					"source": "helius-cache",
				}).to_string();
				return Ok((Status::Ok, body));
			}
		}
		match self.fetch_price_usd(mint).await? {
			Some(usd) => {
				self.cache.insert(mint.to_string(), PriceCache { usd, stored_at: now });
				let body = serde_json::json!({ "mint": mint, "usd": usd, "source": "helius" }).to_string();
				Ok((Status::Ok, body))
			}
			None => Ok((Status::NoContent, "{}".into())),
		}
	}

	pub fn spawn_watcher(&self, bc: Broadcaster) {
		let this = self.clone();
		let mints: Vec<String> = std::env::var("HELIUS_WATCH_MINTS")
			.ok()
			.map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
			.unwrap_or_else(Vec::new);
		if mints.is_empty() { return; }
		let interval_secs = std::env::var("HELIUS_WATCH_INTERVAL_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(5u64);
		tokio::spawn(async move {
			let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
			loop {
				interval.tick().await;
				for mint in &mints {
					if let Ok((status, body)) = this.get_cached_price(mint).await {
						if status.code == 200 {
							if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body) {
								bc.publish(serde_json::json!({ "type": "helius_price", "price": val }));
							}
						}
					}
				}
			}
		});
	}

	async fn fetch_price_usd(&self, mint: &str) -> AppResult<Option<f64>> {
		if self.api_url.is_empty() { return Err(AppError::BadRequest("HELIUS_API_KEY or HELIUS_RPC_URL not configured".into())); }
		let req = serde_json::json!({
			"jsonrpc": "2.0",
			"id": "1",
			"method": "getAsset",
			"params": { "id": mint }
		});
		let resp = self.client.post(&self.api_url)
			.header("Content-Type", "application/json")
			.json(&req)
			.send()
			.await
			.map_err(|e| AppError::Anyhow(e.into()))?;
		if !resp.status().is_success() { return Ok(None); }
		let val: serde_json::Value = resp.json().await.map_err(|e| AppError::Anyhow(e.into()))?;
		Ok(extract_usd(&val))
	}
}

fn extract_usd(v: &serde_json::Value) -> Option<f64> {
	v.pointer("/result/token_info/price_info/price")
		.and_then(|x| x.as_f64())
		.or_else(|| v.pointer("/result/token_info/price_info/price_per_token").and_then(|x| x.as_f64()))
		.or_else(|| v.pointer("/result/price_info/price").and_then(|x| x.as_f64()))
}
