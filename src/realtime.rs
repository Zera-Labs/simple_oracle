use futures::{Stream, StreamExt};
use rocket::response::stream::{Event, EventStream};
use rocket::serde::json::Json;
use rocket::State;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct Broadcaster {
	inner: Arc<broadcast::Sender<serde_json::Value>>,
}

impl Broadcaster {
	pub fn new() -> Self {
		let (tx, _rx) = broadcast::channel(1024);
		Self { inner: Arc::new(tx) }
	}
	pub fn publish(&self, payload: serde_json::Value) {
		let _ = self.inner.send(payload);
	}
}

#[get("/sse")]
pub async fn sse(bc: &State<Broadcaster>) -> EventStream![] {
	let mut rx = bc.inner.subscribe();
	EventStream! {
		loop {
			match rx.recv().await {
				Ok(msg) => yield Event::json(&msg),
				Err(_) => break,
			}
		}
	}
}

#[get("/ws")]
pub async fn ws_upgrade() -> &'static str {
	// Placeholder; use rocket_ws crate for full websocket if needed
	"WebSocket not implemented in this mock; use /sse for updates"
} 