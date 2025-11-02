use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "zera_smoke")] 
#[command(about = "Smoke test for Zera Oracle endpoints", long_about = None)]
struct Opts {
	#[arg(long, default_value = "http://127.0.0.1:8000")] 
	base: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let opts = Opts::parse();
	let client = reqwest::Client::new();

	println!("[1/3] GET /health");
	let r = client.get(format!("{}/api/v1/health", opts.base)).send().await?;
	println!("  status: {}", r.status());
	anyhow::ensure!(r.status().is_success(), "health failed");

	println!("[2/3] GET /prices");
	let r = client.get(format!("{}/api/v1/prices", opts.base)).send().await?;
	println!("  status: {}", r.status());
	anyhow::ensure!(r.status().is_success(), "prices failed");

	println!("[3/3] GET /qn/search");
	let r = client.get(format!("{}/api/v1/qn/addon/912/search?query=orca", opts.base)).send().await?;
	println!("  status: {}", r.status());
	// Either success via origin/edge or 400/401 depending on env; accept 2xx/4xx here
	anyhow::ensure!(r.status().is_success() || r.status().is_client_error(), "qn search unexpected status");

	println!("OK");
	Ok(())
}


