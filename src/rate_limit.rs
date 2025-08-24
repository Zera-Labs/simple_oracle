use dashmap::DashMap;
use std::time::{Duration, Instant};

pub struct RateLimiter {
	limits: DashMap<String, (u32, Instant)>,
	window: Duration,
	max: u32,
}

impl RateLimiter {
	pub fn new_per_minute(max: u32) -> Self {
		Self { limits: DashMap::new(), window: Duration::from_secs(60), max }
	}
	pub fn check_and_increment(&self, key: &str) -> bool {
		let now = Instant::now();
		let mut entry = self.limits.entry(key.to_string()).or_insert((0, now));
		if now.duration_since(entry.1) > self.window {
			*entry = (0, now);
		}
		if entry.0 >= self.max {
			return false;
		}
		entry.0 += 1;
		true
	}
} 