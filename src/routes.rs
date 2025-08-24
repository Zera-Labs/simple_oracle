use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::{Route, State};
use serde::Deserialize;
use serde_json::json;

use crate::auth::AuthUser;
use crate::db::DbState;
use crate::errors::{AppError, AppResult};
use crate::models::{Config, PaginatedAuditResponse, Price, SymbolMap};
use crate::rate_limit::RateLimiter;
use crate::realtime::Broadcaster;

#[get("/health")]
pub fn health() -> Json<serde_json::Value> {
	Json(json!({"status": "ok", "ts": Price::now_iso()}))
}

#[get("/prices")]
pub fn list_prices(db: &State<DbState>) -> AppResult<Json<Vec<Price>>> {
	Ok(Json(db.list_prices()?))
}

#[get("/prices/<mint>")]
pub fn get_price(mint: &str, db: &State<DbState>) -> AppResult<Json<Price>> {
	Ok(Json(db.get_price(mint)?))
}

#[derive(Debug, Deserialize)]
struct UpsertPriceBody {
	mint: String,
	#[serde(default)]
	symbol: Option<String>,
	usd_mantissa: String,
	usd_scale: u32,
	#[serde(default)]
	decimals: Option<u8>,
}

#[post("/prices", data = "<body>")]
pub fn upsert_price(
	user: AuthUser,
	db: &State<DbState>,
	bc: &State<Broadcaster>,
	limiter: &State<RateLimiter>,
	body: Json<UpsertPriceBody>,
) -> AppResult<(Status, Json<Price>)> {
	user.require_admin()?;
	if !limiter.check_and_increment(&user.subject) { return Err(AppError::TooManyRequests); }
	let now = Price::now_iso();
	let price = Price {
		mint: body.mint.clone(),
		symbol: body.symbol.clone(),
		usd_mantissa: body.usd_mantissa.clone(),
		usd_scale: body.usd_scale,
		updated_at: now,
		updated_by: format!("admin:{}", user.subject),
		decimals: body.decimals,
	};
	let saved = db.upsert_price(&price, &user.subject)?;
	bc.publish(json!({"type":"price_upsert","price": saved}));
	Ok((Status::Created, Json(saved)))
}

#[patch("/prices/<mint>", data = "<patch>")]
pub fn patch_price(user: AuthUser, db: &State<DbState>, bc: &State<Broadcaster>, limiter: &State<RateLimiter>, mint: &str, patch: Json<serde_json::Value>) -> AppResult<Json<Price>> {
	user.require_admin()?;
	if !limiter.check_and_increment(&user.subject) { return Err(AppError::TooManyRequests); }
	let updated = db.patch_price(mint, patch.into_inner(), &user.subject)?;
	bc.publish(json!({"type":"price_patch","mint": mint, "price": updated}));
	Ok(Json(updated))
}

#[delete("/prices/<mint>")]
pub fn delete_price(user: AuthUser, db: &State<DbState>, bc: &State<Broadcaster>, limiter: &State<RateLimiter>, mint: &str) -> AppResult<Status> {
	user.require_admin()?;
	if !limiter.check_and_increment(&user.subject) { return Err(AppError::TooManyRequests); }
	db.delete_price(mint, &user.subject)?;
	bc.publish(json!({"type":"price_delete","mint": mint}));
	Ok(Status::NoContent)
}

#[get("/symbols")]
pub fn get_symbols(db: &State<DbState>) -> AppResult<Json<Vec<SymbolMap>>> {
	Ok(Json(db.list_symbols()?))
}

#[derive(Debug, Deserialize)]
struct UpsertSymbolBody { symbol: String, mint: String }

#[post("/symbols", data = "<body>")]
pub fn upsert_symbol(user: AuthUser, db: &State<DbState>, bc: &State<Broadcaster>, limiter: &State<RateLimiter>, body: Json<UpsertSymbolBody>) -> AppResult<Status> {
	user.require_admin()?;
	if !limiter.check_and_increment(&user.subject) { return Err(AppError::TooManyRequests); }
	db.upsert_symbol(&body.symbol, &body.mint)?;
	bc.publish(json!({"type":"symbol_upsert","symbol": body.symbol, "mint": body.mint}));
	Ok(Status::Created)
}

#[get("/config")]
pub fn get_config(db: &State<DbState>) -> AppResult<Json<Config>> {
	Ok(Json(db.get_config()?))
}

#[patch("/config", data = "<patch>")]
pub fn patch_config(user: AuthUser, db: &State<DbState>, bc: &State<Broadcaster>, limiter: &State<RateLimiter>, patch: Json<serde_json::Value>) -> AppResult<Json<Config>> {
	user.require_admin()?;
	if !limiter.check_and_increment(&user.subject) { return Err(AppError::TooManyRequests); }
	let cfg = db.update_config(patch.into_inner(), &user.subject)?;
	bc.publish(json!({"type":"config_patch","config": cfg}));
	Ok(Json(cfg))
}

#[get("/audit?<limit>&<cursor>")]
pub fn get_audit(db: &State<DbState>, limit: Option<usize>, cursor: Option<String>) -> AppResult<Json<PaginatedAuditResponse>> {
	let limit = limit.unwrap_or(100).min(500);
	let (entries, next) = db.list_audit(limit, cursor)?;
	Ok(Json(PaginatedAuditResponse { entries, next_cursor: next }))
}

#[get("/prices/_examples")]
pub fn examples() -> Json<serde_json::Value> {
	Json(json!({
		"examples": [
			{"mint":"GkN1...","symbol":"USDC","usd_mantissa":"100","usd_scale":2,"decimals":6},
			{"mint":"3ZaR...","symbol":"ZERA","usd_mantissa":"10","usd_scale":2,"decimals":6}
		]
	}))
}

pub fn mount_routes() -> Vec<Route> {
	routes![
		health,
		// prices
		list_prices,
		get_price,
		upsert_price,
		patch_price,
		delete_price,
		// symbols
		get_symbols,
		upsert_symbol,
		// config
		get_config,
		patch_config,
		// audit
		get_audit,
		// examples
		examples,
		// realtime
		crate::realtime::sse,
		crate::realtime::ws_upgrade,
	]
} 