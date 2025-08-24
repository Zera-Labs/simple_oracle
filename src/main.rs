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

use dotenvy::dotenv;
use rocket_cors::{AllowedHeaders, AllowedMethods, AllowedOrigins, CorsOptions};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::db::DbState;
use crate::models::Price;
use crate::rate_limit::RateLimiter;
use crate::realtime::Broadcaster;
use crate::routes::mount_routes;

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
	let broadcaster = Broadcaster::new();
	let limiter = RateLimiter::new_per_minute(std::env::var("WRITE_RATE_LIMIT_PER_MINUTE").ok().and_then(|v| v.parse().ok()).unwrap_or(60));

	let cors = build_cors();

	let rocket = rocket::build()
		.manage(db)
		.manage(broadcaster)
		.manage(limiter)
		.attach(cors)
		.mount("/api/v1", mount_routes());

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
