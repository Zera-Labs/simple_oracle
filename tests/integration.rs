use std::time::Duration;

#[tokio::test]
async fn health_works() {
    let base = std::env::var("BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8000".into());
    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap();
    let r = client.get(format!("{}/api/v1/health", base)).send().await.unwrap();
    assert!(r.status().is_success());
}

#[tokio::test]
async fn prices_crud_requires_auth_for_writes() {
    let base = std::env::var("BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8000".into());
    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap();
    let body = serde_json::json!({
        "mint":"TestMint1111111111111111111111111111111111111",
        "symbol":"TEST",
        "usd_mantissa":"123",
        "usd_scale":2,
        "decimals":6
    });
    let r = client.post(format!("{}/api/v1/prices", base)).json(&body).send().await.unwrap();
    assert!(r.status() == reqwest::StatusCode::FORBIDDEN || r.status() == reqwest::StatusCode::UNAUTHORIZED);
}


