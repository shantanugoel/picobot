use std::fs;

use picobot::config::Config;
use picobot::models::router::ModelRegistry;

fn main() {
    let config = load_config().unwrap_or_default();
    if let Err(err) = ModelRegistry::from_config(&config) {
        eprintln!("Model registry error: {err}");
    }
    println!("PicoBot initializing...");
}

fn load_config() -> Option<Config> {
    let raw = fs::read_to_string("config.toml").ok()?;
    toml::from_str(&raw).ok()
}
