#![allow(clippy::result_large_err)]

#[macro_use]
extern crate rocket;

mod auth;
mod db;
mod errors;
mod models;
mod rate_limit;
mod routes;
mod realtime;
mod qn_proxy;
mod helius;

use dotenvy::dotenv;
use rocket::fairing::AdHoc;
use rocket_cors::{AllowedHeaders, AllowedMethods, AllowedOrigins, CorsOptions};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::db::DbState;
use crate::models::Price;
use crate::rate_limit::RateLimiter;
use crate::qn_proxy::QuicknodeProxy;
use crate::realtime::Broadcaster;
use crate::routes::mount_routes;
use crate::helius::HeliusPriceService;

#[launch]
fn rocket() -> _ {
	// init logging early
	let env_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info,rocket=info".into());
	tracing_subscriber::registry()
		.with(tracing_subscriber::EnvFilter::new(env_filter))
		.with(tracing_subscriber::fmt::layer())
		.init();

	dotenv().ok();

	let db = DbState::initialize().expect("failed to init database");
	seed_fixtures(&db);
	spawn_pegger_if_configured(db.clone());
	let broadcaster = Broadcaster::new();
	let limiter = RateLimiter::new_per_minute(std::env::var("WRITE_RATE_LIMIT_PER_MINUTE").ok().and_then(|v| v.parse().ok()).unwrap_or(60));

	let cors = build_cors();

	let rocket = rocket::build()
		.manage(db.clone())
		.manage(broadcaster)
		.manage(QuicknodeProxy::from_env())
		.manage(HeliusPriceService::from_env())
		.manage(limiter)
		.attach(cors)
		.mount("/api/v1", mount_routes())
		.attach(AdHoc::on_liftoff("hotset refresher", |rocket| Box::pin(async move {
			let db = rocket.state::<DbState>().cloned();
			let proxy = rocket.state::<QuicknodeProxy>();
			let bc = rocket.state::<Broadcaster>().cloned();
			let helius = rocket.state::<HeliusPriceService>().cloned();
			if let (Some(db), Some(proxy)) = (db, proxy) {
				if std::env::var("QNODE_L2_ENABLED").ok().map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(true) {
					proxy.spawn_hotset_refresher(db);
				}
			}
			if let (Some(helius), Some(bc)) = (helius, bc) {
				helius.spawn_watcher(bc);
			}
		})));

	rocket
}

fn build_cors() -> rocket_cors::Cors {
	let allowed_origins = AllowedOrigins::all();
	let allowed_methods: AllowedMethods = [
		rocket::http::Method::Get,
		rocket::http::Method::Post,
		rocket::http::Method::Patch,
		rocket::http::Method::Delete,
		rocket::http::Method::Options,
	]
	.into_iter()
	.map(From::from)
	.collect();

	CorsOptions {
		allowed_origins,
		allowed_methods,
		allowed_headers: AllowedHeaders::all(),
		allow_credentials: true,
		..Default::default()
	}
	.to_cors()
	.expect("CORS configuration must be valid")
}

fn seed_fixtures(db: &DbState) {
	let usdc_mint = std::env::var("USDC_DEVNET_MINT").ok();
	let zera_mint = std::env::var("ZERA_DEVNET_MINT").ok();
	if let Some(mint) = usdc_mint {
		let price = Price {
			mint: mint.clone(),
			symbol: Some("USDC".into()),
			usd_mantissa: "100".into(),
			usd_scale: 2,
			updated_at: Price::now_iso(),
			updated_by: "seed".into(),
			decimals: Some(6),
		};
		let _ = db.upsert_price(&price, "seed");
	}
	if let Some(mint) = zera_mint {
		let price = Price {
			mint: mint.clone(),
			symbol: Some("ZERA".into()),
			usd_mantissa: "10".into(),
			usd_scale: 2,
			updated_at: Price::now_iso(),
			updated_by: "seed".into(),
			decimals: Some(6),
		};
		let _ = db.upsert_price(&price, "seed");
	}
}

fn spawn_pegger_if_configured(db: DbState) {
	let sources = std::env::var("PEG_SOURCES").ok();
	if sources.is_none() { return; }
	let sources = sources.unwrap();
	if sources.trim().is_empty() { return; }
	tokio::spawn(async move {
		let client = reqwest::Client::new();
		let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
		let parsed: Vec<_> = sources.split(';').filter(|s| !s.trim().is_empty()).collect();
		loop {
			interval.tick().await;
			for src in &parsed {
				// Format: mint|url|jsonPointer|scale
				let parts: Vec<&str> = src.split('|').collect();
				if parts.len() < 4 { continue; }
				let mint = parts[0].to_string();
				let url = parts[1];
				let pointer = parts[2];
				let scale: u32 = parts[3].parse().unwrap_or(2);
				if let Ok(resp) = client.get(url).send().await {
					if let Ok(val) = resp.json::<serde_json::Value>().await {
						let mut cur = &val;
						for key in pointer.split('.') { if let Some(v) = cur.get(key) { cur = v; } }
						if let Some(price_num) = cur.as_f64() {
							let mantissa = ((price_num * 10f64.powi(scale as i32)).round() as i128).to_string();
							let price = Price {
								mint: mint.clone(),
								symbol: None,
								usd_mantissa: mantissa,
								usd_scale: scale,
								updated_at: Price::now_iso(),
								updated_by: "pegger".into(),
								decimals: None,
							};
							let _ = db.upsert_price(&price, "pegger");
						}
					}
				}
			}
		}
	});
}
