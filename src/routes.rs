use rocket::http::Status;
use rocket::response::content::RawHtml;
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
use crate::qn_proxy::QuicknodeProxy;
use crate::helius::HeliusPriceService;

#[get("/health")]
pub fn health() -> Json<serde_json::Value> {
	Json(json!({"status": "ok", "ts": Price::now_iso()}))
}

#[post("/admin/login", data = "<body>")]
pub fn admin_login(body: Json<serde_json::Value>) -> AppResult<Json<serde_json::Value>> {
	let password = std::env::var("ADMIN_UI_PASSWORD").unwrap_or_default();
	let provided = body.get("password").and_then(|v| v.as_str()).unwrap_or("");
	if provided != password || provided.is_empty() { return Err(AppError::Unauthorized); }
	let sub = body.get("user").and_then(|v| v.as_str()).unwrap_or("ops");
	let exp = (time::OffsetDateTime::now_utc().unix_timestamp() + 3600) as usize;
	let claims = json!({"sub": sub, "role": "admin", "exp": exp});
	let secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".into());
	let token = jsonwebtoken::encode(&jsonwebtoken::Header::default(), &claims, &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()))
		.map_err(|e| AppError::Anyhow(e.into()))?;
	Ok(Json(json!({"token": token})))
}

#[get("/admin")]
pub fn admin_page() -> RawHtml<&'static str> {
	RawHtml(r#"<!doctype html><html><head><meta charset='utf-8'/><meta name='viewport' content='width=device-width,initial-scale=1'/><title>Zera Oracle Admin</title><style>body{font-family:sans-serif;max-width:900px;margin:24px auto;padding:0 12px}table{border-collapse:collapse;width:100%}td,th{border:1px solid #ddd;padding:8px}input,button{padding:8px;margin:4px}#login{margin-bottom:16px;border:1px solid #ccc;padding:12px;border-radius:8px}</style></head><body><h2>Zera Devnet Oracle — Admin</h2><div id='login'><input id='user' placeholder='user'/> <input id='pwd' placeholder='password' type='password'/> <button onclick='login()'>Login</button> <span id='status'></span></div><div><button onclick='loadPrices()'>Refresh</button> <button onclick='addPrice()'>Add/Upsert</button></div><table id='tbl'><thead><tr><th>mint</th><th>symbol</th><th>mantissa</th><th>scale</th><th>decimals</th><th>updated</th><th>by</th><th>actions</th></tr></thead><tbody></tbody></table><script>let token=localStorage.getItem('jwt')||'';function setStatus(t){document.getElementById('status').innerText=t;}async function login(){const user=document.getElementById('user').value;const password=document.getElementById('pwd').value;const r=await fetch('/api/v1/admin/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({user,password})});if(r.ok){const j=await r.json();token=j.token;localStorage.setItem('jwt',token);setStatus('ok');loadPrices();}else setStatus('login failed');}async function loadPrices(){const r=await fetch('/api/v1/prices');const rows=await r.json();const tb=document.querySelector('#tbl tbody');tb.innerHTML='';rows.forEach(p=>{const tr=document.createElement('tr');tr.innerHTML=`<td>${p.mint}</td><td>${p.symbol||''}</td><td>${p.usd_mantissa}</td><td>${p.usd_scale}</td><td>${p.decimals??''}</td><td>${p.updated_at}</td><td>${p.updated_by}</td><td><button onclick='edit("${p.mint}")'>Edit</button><button onclick='delp("${p.mint}")'>Delete</button></td>`;tb.appendChild(tr);});}
function edit(m){const symbol=prompt('symbol (opt)');const usd_mantissa=prompt('usd_mantissa (string)');const usd_scale=parseInt(prompt('usd_scale (u32)')||'0');const decimals=prompt('decimals (opt)');const body={};if(symbol!==null&&symbol!=='')body.symbol=symbol;if(usd_mantissa)body.usd_mantissa=usd_mantissa;if(!isNaN(usd_scale))body.usd_scale=usd_scale;if(decimals)body.decimals=parseInt(decimals);fetch(`/api/v1/prices/${m}`,{method:'PATCH',headers:{'Content-Type':'application/json','Authorization':`Bearer ${token}`},body:JSON.stringify(body)}).then(()=>loadPrices());}
function delp(m){fetch(`/api/v1/prices/${m}`,{method:'DELETE',headers:{'Authorization':`Bearer ${token}`}}).then(()=>loadPrices());}
function addPrice(){const mint=prompt('mint');if(!mint)return;const symbol=prompt('symbol');const usd_mantissa=prompt('usd_mantissa');const usd_scale=parseInt(prompt('usd_scale')||'2');const decimals=parseInt(prompt('decimals')||'6');const body={mint,symbol,usd_mantissa,usd_scale,decimals};fetch('/api/v1/prices',{method:'POST',headers:{'Content-Type':'application/json','Authorization':`Bearer ${token}`},body:JSON.stringify(body)}).then(()=>loadPrices());}
</script></body></html>"#)
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
		admin_login,
		admin_page,
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
        // quicknode proxy
        qn_dexes,
        qn_pools,
        qn_dex_pools,
        qn_pool_by_address,
        qn_token_pools,
        qn_token,
        qn_search,
        qn_tokens_aggregate,
        // helius
        helius_price,
	]
} 
#[get("/helius/price/<mint>")]
pub async fn helius_price(helius: &State<HeliusPriceService>, mint: &str) -> (Status, String) {
    match helius.get_cached_price(mint).await {
        Ok((s, b)) => (s, b),
        Err(e) => (e.status(), json!({"error": e.to_string()}).to_string()),
    }
}

// ========================= QuickNode pass-through (cached) =========================

#[get("/qn/addon/912/networks/solana/dexes?<page>&<limit>&<sort>&<order_by>")]
pub async fn qn_dexes(proxy: &State<QuicknodeProxy>, db: &State<DbState>, page: Option<String>, limit: Option<String>, sort: Option<String>, order_by: Option<String>) -> (Status, String) {
    let params = vec![
        opt("page", page),
        opt("limit", limit),
        opt("sort", sort),
        opt("order_by", order_by),
    ].into_iter().flatten().collect::<Vec<_>>();
    match proxy.get_cached(Some(db), "addon/912/networks/solana/dexes", &params).await {
        Ok((s, b)) => (s, b),
        Err(e) => (e.status(), json!({"error": e.to_string()}).to_string()),
    }
}

#[get("/qn/addon/912/networks/solana/pools?<page>&<limit>&<sort>&<order_by>")]
pub async fn qn_pools(proxy: &State<QuicknodeProxy>, db: &State<DbState>, page: Option<String>, limit: Option<String>, sort: Option<String>, order_by: Option<String>) -> (Status, String) {
    let params = vec![
        opt("page", page),
        opt("limit", limit),
        opt("sort", sort),
        opt("order_by", order_by),
    ].into_iter().flatten().collect::<Vec<_>>();
    match proxy.get_cached(Some(db), "addon/912/networks/solana/pools", &params).await {
        Ok((s, b)) => (s, b),
        Err(e) => (e.status(), json!({"error": e.to_string()}).to_string()),
    }
}

#[get("/qn/addon/912/networks/solana/dexes/<dex>/pools?<page>&<limit>&<sort>&<order_by>")]
pub async fn qn_dex_pools(proxy: &State<QuicknodeProxy>, db: &State<DbState>, dex: &str, page: Option<String>, limit: Option<String>, sort: Option<String>, order_by: Option<String>) -> (Status, String) {
    let params = vec![
        opt("page", page),
        opt("limit", limit),
        opt("sort", sort),
        opt("order_by", order_by),
    ].into_iter().flatten().collect::<Vec<_>>();
    let path = format!("addon/912/networks/solana/dexes/{}/pools", dex);
    match proxy.get_cached(Some(db), &path, &params).await {
        Ok((s, b)) => (s, b),
        Err(e) => (e.status(), json!({"error": e.to_string()}).to_string()),
    }
}

#[get("/qn/addon/912/networks/solana/pools/<pool_address>?<inversed>")]
pub async fn qn_pool_by_address(proxy: &State<QuicknodeProxy>, db: &State<DbState>, pool_address: &str, inversed: Option<String>) -> (Status, String) {
    let params = vec![ opt("inversed", inversed) ].into_iter().flatten().collect::<Vec<_>>();
    let path = format!("addon/912/networks/solana/pools/{}", pool_address);
    match proxy.get_cached(Some(db), &path, &params).await {
        Ok((s, b)) => (s, b),
        Err(e) => (e.status(), json!({"error": e.to_string()}).to_string()),
    }
}

#[get("/qn/addon/912/networks/solana/tokens/<token_address>/pools?<sort>&<order_by>&<address>")]
pub async fn qn_token_pools(proxy: &State<QuicknodeProxy>, db: &State<DbState>, token_address: &str, sort: Option<String>, order_by: Option<String>, address: Option<String>) -> (Status, String) {
    let params = vec![
        opt("sort", sort),
        opt("order_by", order_by),
        opt("address", address),
    ].into_iter().flatten().collect::<Vec<_>>();
    let path = format!("addon/912/networks/solana/tokens/{}/pools", token_address);
    match proxy.get_cached(Some(db), &path, &params).await {
        Ok((s, b)) => (s, b),
        Err(e) => (e.status(), json!({"error": e.to_string()}).to_string()),
    }
}

#[get("/qn/addon/912/networks/solana/tokens/<token_address>")]
pub async fn qn_token(proxy: &State<QuicknodeProxy>, db: &State<DbState>, token_address: &str) -> (Status, String) {
    let path = format!("addon/912/networks/solana/tokens/{}", token_address);
    match proxy.get_cached(Some(db), &path, &[]).await {
        Ok((s, b)) => (s, b),
        Err(e) => (e.status(), json!({"error": e.to_string()}).to_string()),
    }
}

#[get("/qn/addon/912/search?<query>")]
pub async fn qn_search(proxy: &State<QuicknodeProxy>, db: &State<DbState>, query: Option<String>) -> (Status, String) {
    let params = vec![ opt("query", query) ].into_iter().flatten().collect::<Vec<_>>();
    match proxy.get_cached(Some(db), "addon/912/search", &params).await {
        Ok((s, b)) => (s, b),
        Err(e) => (e.status(), json!({"error": e.to_string()}).to_string()),
    }
}

fn opt(k: &str, v: Option<String>) -> Option<(String, String)> {
    v.map(|vv| (k.to_string(), vv))
}

// Aggregate endpoint: fetch multiple token datas with coalescing and concurrency caps
#[get("/qn/tokens?<addresses>")]
pub async fn qn_tokens_aggregate(proxy: &State<QuicknodeProxy>, db: &State<DbState>, addresses: Option<String>) -> (Status, String) {
    let list = addresses.unwrap_or_default();
    if list.trim().is_empty() { return (Status::BadRequest, json!({"error":"addresses required"}).to_string()); }
    let addrs: Vec<String> = list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(addrs.len());
    let mut tasks: Vec<_> = Vec::new();
    for addr in addrs {
        let path: String = format!("addon/912/networks/solana/tokens/{}", addr);
        tasks.push(async move {
            // The future owns `path`, preventing lifetime issues
            proxy.get_cached(Some(db), &path, &[]).await
        });
    }
    for t in futures::future::join_all(tasks).await {
        match t {
            Ok((_s, body)) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) { out.push(v); }
            }
            Err(_) => {}
        }
    }
    (Status::Ok, serde_json::to_string(&out).unwrap_or("[]".into()))
}