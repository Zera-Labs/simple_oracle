use dashmap::DashMap;
use rocket::http::Status;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore, oneshot};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION};

use crate::db::DbState;
use crate::errors::{AppError, AppResult};

#[derive(Clone)]
struct CachedEntry {
	status: Status,
	body: String,
	stored_at: Instant,
}

pub struct QuicknodeProxy {
	client: reqwest::Client,
	base_url: String,
	cache: DashMap<String, CachedEntry>,
	popularity: DashMap<String, f64>,
	ttl_hot: Duration,
	ttl_warm: Duration,
	ttl_cold: Duration,
	max_stale: Duration,
	enable_l2: bool,
	inflight: Arc<Mutex<HashMap<String, Vec<oneshot::Sender<Result<(Status, String), AppError>>>>>>,
	concurrency: Arc<Semaphore>,
	budget: Arc<Mutex<BudgetState>>,
}

struct BudgetState {
	capacity_per_minute: u32,
	remaining: u32,
	reset_at: Instant,
}

impl QuicknodeProxy {
	pub fn from_env() -> Self {
		let base_url = std::env::var("QNODE_BASE_URL").unwrap_or_default();
		let ttl_hot = std::env::var("QNODE_TTL_HOT_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(15);
		let ttl_warm = std::env::var("QNODE_TTL_WARM_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(45);
		let ttl_cold = std::env::var("QNODE_TTL_COLD_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(300);
		let max_stale = std::env::var("QNODE_MAX_STALE_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(180);
		let timeout_ms = std::env::var("QNODE_TIMEOUT_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(8000);
		let enable_l2 = std::env::var("QNODE_L2_ENABLED").ok().map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(true);
		let concurrency_limit = std::env::var("QNODE_CONCURRENCY").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(16);
		let budget_per_min = std::env::var("QNODE_PER_MINUTE_BUDGET").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(300);
		let mut default_headers = HeaderMap::new();
		default_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
		if let Ok(key) = std::env::var("QNODE_API_KEY") {
			if let Ok(v) = HeaderValue::from_str(&key) {
				default_headers.insert(HeaderName::from_static("x-api-key"), v);
			}
		}
		if let Ok(token) = std::env::var("QNODE_BEARER_TOKEN") {
			let val = format!("Bearer {}", token);
			if let Ok(v) = HeaderValue::from_str(&val) { default_headers.insert(AUTHORIZATION, v); }
		}
		if let Ok(extra) = std::env::var("QNODE_HEADERS") {
			for part in extra.split(';') {
				let mut it = part.splitn(2, ':');
				let name = it.next().map(|s| s.trim()).unwrap_or("");
				let value = it.next().map(|s| s.trim()).unwrap_or("");
				if name.is_empty() || value.is_empty() { continue; }
				if let (Ok(n), Ok(v)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value)) {
					default_headers.insert(n, v);
				}
			}
		}
		let client = reqwest::Client::builder()
			.user_agent("zera-oracle-proxy/1.0")
			.timeout(Duration::from_millis(timeout_ms))
			.default_headers(default_headers)
			.build()
			.expect("failed to build reqwest client");
		Self {
			client,
			base_url: ensure_trailing_slash(base_url),
			cache: DashMap::new(),
			popularity: DashMap::new(),
			ttl_hot: Duration::from_secs(ttl_hot),
			ttl_warm: Duration::from_secs(ttl_warm),
			ttl_cold: Duration::from_secs(ttl_cold),
			max_stale: Duration::from_secs(max_stale),
			enable_l2,
			inflight: Arc::new(Mutex::new(HashMap::new())),
			concurrency: Arc::new(Semaphore::new(concurrency_limit)),
			budget: Arc::new(Mutex::new(BudgetState { capacity_per_minute: budget_per_min, remaining: budget_per_min, reset_at: Instant::now() + Duration::from_secs(60) })),
		}
	}

	pub async fn get_cached(&self, db: Option<&DbState>, path: &str, params: &[(String, String)]) -> AppResult<(Status, String)> {
		let key = Self::make_cache_key("GET", path, params);
		let now = Instant::now();
		self.bump_popularity(&key);
		let ttl = self.choose_ttl(&key);
		if let Some(entry) = self.cache.get(&key) {
			if now.duration_since(entry.stored_at) < ttl {
				return Ok((entry.status, entry.body.clone()));
			}
		}
		if let Some(db) = db.filter(|_| self.enable_l2) {
			let now_epoch = epoch_seconds();
			if let Ok(Some((st, body, expires_at))) = db.http_cache_get(&key, now_epoch) {
				if (expires_at - now_epoch) >= 0 {
					let status = Status::from_code(st as u16).unwrap_or(Status::Ok);
					self.cache.insert(key.clone(), CachedEntry { status, body: body.clone(), stored_at: now });
					return Ok((status, body));
				}
				if now_epoch - expires_at <= self.max_stale.as_secs() as i64 {
					let status = Status::from_code(st as u16).unwrap_or(Status::Ok);
					self.spawn_refresh(db.clone(), key.clone(), path.to_string(), params.to_vec());
					return Ok((status, body));
				}
			}
		}
		self.fetch_singleflight(db, key, path, params).await
	}

	fn build_url(&self, path: &str, params: &[(String, String)]) -> AppResult<reqwest::Url> {
		if self.base_url.is_empty() {
			return Err(AppError::BadRequest("QNODE_BASE_URL not configured".into()));
		}
		let mut url = reqwest::Url::parse(&self.base_url)
			.map_err(|e| AppError::Anyhow(e.into()))?;
		url.set_path(&format!("{}{}", url.path().trim_end_matches('/'), ensure_no_leading_slash(path)));
		{
			let mut qp = url.query_pairs_mut();
			for (k, v) in params {
				qp.append_pair(k, v);
			}
		}
		Ok(url)
	}

	fn make_cache_key(method: &str, path: &str, params: &[(String, String)]) -> String {
		let mut sorted = params.to_vec();
		sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
		let qs = sorted.into_iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("&");
		format!("{}|{}?{}", method, path, qs)
	}

	fn choose_ttl(&self, key: &str) -> Duration {
		let hot_threshold = std::env::var("QNODE_POP_HOT").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(50.0);
		let warm_threshold = std::env::var("QNODE_POP_WARM").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(10.0);
		let p = self.popularity.get(key).map(|e| *e.value()).unwrap_or(0.0);
		if p >= hot_threshold { return self.ttl_hot; }
		if p >= warm_threshold { return self.ttl_warm; }
		self.class_base_ttl(key)
	}

	fn class_base_ttl(&self, key: &str) -> Duration {
		if key.contains("/tokens/") { return self.ttl_warm; }
		if key.contains("/pools/") { return self.ttl_warm; }
		if key.contains("/dexes") { return self.ttl_cold; }
		if key.contains("/search") { return self.ttl_cold; }
		self.ttl_warm
	}

	fn bump_popularity(&self, key: &str) {
		let mut entry = self.popularity.entry(key.to_string()).or_insert(0.0);
		let v = *entry + 1.0;
		*entry = v.min(1_000_000.0);
	}

	fn spawn_refresh(&self, db: DbState, key: String, path: String, params: Vec<(String, String)>) {
		let this = self.clone_shallow();
		tokio::spawn(async move {
			let _ = this.fetch_singleflight(Some(&db), key, &path, &params).await;
		});
	}

	async fn fetch_singleflight(&self, db: Option<&DbState>, key: String, path: &str, params: &[(String, String)]) -> AppResult<(Status, String)> {
		let (rx_opt, leader) = {
			let mut map = self.inflight.lock().await;
			if let Some(waiters) = map.get_mut(&key) {
				let (tx, rx) = oneshot::channel();
				waiters.push(tx);
				(Some(rx), false)
			} else {
				map.insert(key.clone(), Vec::new());
				(None, true)
			}
		};
		if !leader {
			if let Some(rx) = rx_opt {
				match rx.await {
					Ok(Ok((s, b))) => return Ok((s, b)),
					Ok(Err(e)) => return Err(e),
					Err(_) => return Err(AppError::Anyhow(anyhow::anyhow!("singleflight canceled"))),
				}
			}
		}
		let permit = self.concurrency.clone().acquire_owned().await.unwrap();
		if !self.try_consume_budget(1).await {
			drop(permit);
			if let Some(db) = db.filter(|_| self.enable_l2) {
				if let Ok(Some((st, body, _))) = db.http_cache_get(&key, epoch_seconds()) {
					let status = Status::from_code(st as u16).unwrap_or(Status::Ok);
					self.finish_flight(key, Ok((status, body.clone()))).await;
					return Ok((status, body));
				}
			}
			self.finish_flight(key, Err(AppError::TooManyRequests)).await;
			return Err(AppError::TooManyRequests);
		}
		let url = self.build_url(path, params)?;
		let resp = self.client.get(url).send().await.map_err(|e| AppError::Anyhow(e.into()))?;
		let status = Status::from_code(resp.status().as_u16()).unwrap_or(Status::InternalServerError);
		let body = resp.text().await.map_err(|e| AppError::Anyhow(e.into()))?;
		drop(permit);
		let now = Instant::now();
		self.cache.insert(key.clone(), CachedEntry { status, body: body.clone(), stored_at: now });
		if let Some(db) = db.filter(|_| self.enable_l2) {
			let ttl = self.choose_ttl(&key);
			let _ = db.http_cache_put(&key, status.code, &body, ttl.as_secs() as i64, epoch_seconds());
		}
		self.finish_flight(key, Ok((status, body.clone()))).await;
		Ok((status, body))
	}

	async fn finish_flight(&self, key: String, result: Result<(Status, String), AppError>) {
		let waiters = {
			let mut map = self.inflight.lock().await;
			map.remove(&key).unwrap_or_default()
		};
		match result {
			Ok((status, body)) => {
				for tx in waiters {
					let _ = tx.send(Ok((status, body.clone())));
				}
			}
			Err(err) => {
				let msg = err.to_string();
				let mut it = waiters.into_iter();
				if let Some(tx0) = it.next() {
					let _ = tx0.send(Err(err));
				}
				for tx in it {
					let _ = tx.send(Err(AppError::Anyhow(anyhow::anyhow!(msg.clone()))));
				}
			}
		}
	}

	async fn try_consume_budget(&self, n: u32) -> bool {
		let mut b = self.budget.lock().await;
		let now = Instant::now();
		if now >= b.reset_at {
			b.remaining = b.capacity_per_minute;
			b.reset_at = now + Duration::from_secs(60);
		}
		if b.remaining < n { return false; }
		b.remaining -= n;
		true
	}

	fn clone_shallow(&self) -> Self {
		Self {
			client: self.client.clone(),
			base_url: self.base_url.clone(),
			cache: self.cache.clone(),
			popularity: self.popularity.clone(),
			ttl_hot: self.ttl_hot,
			ttl_warm: self.ttl_warm,
			ttl_cold: self.ttl_cold,
			max_stale: self.max_stale,
			enable_l2: self.enable_l2,
			inflight: self.inflight.clone(),
			concurrency: self.concurrency.clone(),
			budget: self.budget.clone(),
		}
	}

	pub fn spawn_hotset_refresher(&self, db: DbState) {
		let this = self.clone_shallow();
		tokio::spawn(async move {
			let mut interval = tokio::time::interval(Duration::from_secs(20));
			loop {
				interval.tick().await;
				let size = std::env::var("QNODE_HOTSET_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(500usize);
				let keys = db.http_cache_list_hot_keys(size).unwrap_or_default();
				for key in keys {
					if !this.try_consume_budget(1).await { break; }
					if let Some((path, params)) = parse_cache_key(&key) {
						let _ = this.fetch_singleflight(Some(&db), key.clone(), &path, &params).await;
					}
				}
				let _ = db.http_cache_cleanup_expired(epoch_seconds(), 1000);
			}
		});
	}
}

fn ensure_trailing_slash(mut s: String) -> String {
	if !s.ends_with('/') { s.push('/'); }
	s
}

fn ensure_no_leading_slash(path: &str) -> String {
	if path.starts_with('/') { path.trim_start_matches('/').to_string() } else { path.to_string() }
}

fn parse_cache_key(key: &str) -> Option<(String, Vec<(String, String)>)> {
	let mut parts = key.splitn(2, '|');
	let _method = parts.next()?;
	let rest = parts.next()?;
	let mut it = rest.splitn(2, '?');
	let path = it.next()?.to_string();
	let qs = it.next().unwrap_or("");
	let params = if qs.is_empty() { Vec::new() } else {
		qs.split('&').filter_map(|p| {
			let mut kv = p.splitn(2, '=');
			let k = kv.next()?;
			let v = kv.next().unwrap_or("");
			Some((k.to_string(), v.to_string()))
		}).collect::<Vec<_>>()
	};
	Some((path, params))
}

fn epoch_seconds() -> i64 { (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()) as i64 }


