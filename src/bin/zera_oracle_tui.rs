use clap::Parser;
use dialoguer::{Confirm, Input, Password};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
struct Args {
	#[arg(long, default_value = "http://127.0.0.1:8000")] base: String,
	#[arg(long, default_value = "ops")] user: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Price { mint: String, symbol: Option<String>, usd_mantissa: String, usd_scale: u32, updated_at: String, updated_by: String, decimals: Option<u8> }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let args = Args::parse();
	let base = args.base.trim_end_matches('/');
	let client = reqwest::Client::new();
	println!("Zera Oracle TUI — {}", base);
	let pwd = Password::new().with_prompt("Admin password").interact()?;
	let token = login(&client, base, &args.user, &pwd).await?;
	loop {
		let prices = list_prices(&client, base).await.unwrap_or_default();
		println!("\nPrices ({}):", prices.len());
		for p in &prices { println!("- {} {} mantissa={} scale={} dec={:?}", p.mint, p.symbol.clone().unwrap_or_default(), p.usd_mantissa, p.usd_scale, p.decimals); }
		if !Confirm::new().with_prompt("Edit a price?").default(false).interact()? { break; }
		let mint: String = Input::new().with_prompt("mint").interact_text()?;
		let symbol: String = Input::new().with_prompt("symbol (empty=skip)").allow_empty(true).interact_text()?;
		let mantissa: String = Input::new().with_prompt("usd_mantissa (string, empty=skip)").allow_empty(true).interact_text()?;
		let scale_s: String = Input::new().with_prompt("usd_scale (u32, empty=skip)").allow_empty(true).interact_text()?;
		let decimals_s: String = Input::new().with_prompt("decimals (u8, empty=skip)").allow_empty(true).interact_text()?;
		let mut body = serde_json::Map::new();
		if !symbol.is_empty() { body.insert("symbol".into(), serde_json::Value::String(symbol)); }
		if !mantissa.is_empty() { body.insert("usd_mantissa".into(), serde_json::Value::String(mantissa)); }
		if let Ok(v) = scale_s.parse::<u32>() { body.insert("usd_scale".into(), serde_json::Value::from(v)); }
		if let Ok(v) = decimals_s.parse::<u8>() { body.insert("decimals".into(), serde_json::Value::from(v)); }
		let _ = client.patch(format!("{}/api/v1/prices/{}", base, mint))
			.header("Authorization", format!("Bearer {}", token))
			.json(&serde_json::Value::Object(body)).send().await?;
	}
	Ok(())
}

async fn login(client: &reqwest::Client, base: &str, user: &str, password: &str) -> anyhow::Result<String> {
	let r = client.post(format!("{}/api/v1/admin/login", base)).json(&serde_json::json!({"user": user, "password": password})).send().await?;
	if !r.status().is_success() { anyhow::bail!("login failed: {}", r.status()); }
	let v: serde_json::Value = r.json().await?;
	Ok(v.get("token").and_then(|t| t.as_str()).unwrap_or("").to_string())
}

async fn list_prices(client: &reqwest::Client, base: &str) -> anyhow::Result<Vec<Price>> {
	let r = client.get(format!("{}/api/v1/prices", base)).send().await?;
	Ok(r.json::<Vec<Price>>().await.unwrap_or_default())
} 