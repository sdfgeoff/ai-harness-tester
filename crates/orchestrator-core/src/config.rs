use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    path::Path,
    process::Command,
};

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_results_dir")]
    pub results_dir: String,
    pub models: BTreeMap<String, ModelProfile>,
    pub harnesses: BTreeMap<String, HarnessProfile>,
}

fn default_timeout() -> u64 {
    1800
}

fn default_results_dir() -> String {
    "results".to_owned()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModelProfile {
    pub model_name: String,
    pub base_url: String,
    pub api_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HarnessProfile {
    pub image: String,
}

pub fn load_config(path: &Path) -> Result<Config, String> {
    tracing::info!(config_path = %path.display(), "loading config");
    let file = File::open(path)
        .map_err(|error| format!("failed to open config {}: {error}", path.display()))?;
    let config: Config = serde_json::from_reader(file)
        .map_err(|error| format!("failed to parse config {}: {error}", path.display()))?;
    tracing::info!(
        models = config.models.len(),
        harnesses = config.harnesses.len(),
        timeout_seconds = config.timeout_seconds,
        results_dir = %config.results_dir,
        "config loaded"
    );
    Ok(config)
}

// ── Redacted config snapshot ────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct RedactedConfig<'a> {
    timeout_seconds: u64,
    results_dir: &'a str,
    models: BTreeMap<&'a str, RedactedModelProfile<'a>>,
    harnesses: &'a BTreeMap<String, HarnessProfile>,
}

#[derive(Debug, Serialize)]
struct RedactedModelProfile<'a> {
    model_name: &'a str,
    base_url: &'a str,
    api_key: &'static str,
}

pub fn write_redacted_config_snapshot(batch_dir: &Path, config: &Config) -> Result<(), String> {
    let models = config
        .models
        .iter()
        .map(|(name, profile)| {
            (
                name.as_str(),
                RedactedModelProfile {
                    model_name: &profile.model_name,
                    base_url: &profile.base_url,
                    api_key: "<redacted>",
                },
            )
        })
        .collect();
    let redacted = RedactedConfig {
        timeout_seconds: config.timeout_seconds,
        results_dir: &config.results_dir,
        models,
        harnesses: &config.harnesses,
    };
    let path = batch_dir.join("config.json");
    let file = File::create(&path).map_err(|error| {
        format!(
            "failed to create redacted config snapshot {}: {error}",
            path.display()
        )
    })?;
    serde_json::to_writer_pretty(file, &redacted).map_err(|error| {
        format!(
            "failed to write redacted config snapshot {}: {error}",
            path.display()
        )
    })
}

// ── Preflight helpers ───────────────────────────────────────────────────────

pub fn inspect_docker_image(harness_name: &str, image: &str) -> Result<String, String> {
    tracing::info!(harness = harness_name, image = image, "inspecting Docker image");
    let output = Command::new("docker")
        .arg("image")
        .arg("inspect")
        .arg("--format")
        .arg("{{.Id}}")
        .arg(image)
        .output()
        .map_err(|error| {
            format!("failed to inspect Docker image for harness '{harness_name}': {error}")
        })?;

    if output.status.success() {
        let image_id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok(image_id)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Docker image for harness '{harness_name}' does not exist locally: {image}: {}",
            stderr.trim()
        ))
    }
}

pub fn preflight_model(profile_name: &str, model: &ModelProfile) -> Result<(), String> {
    let models_url = format!("{}/models", model.base_url.trim_end_matches('/'));
    tracing::info!(profile = profile_name, url = %models_url, "preflight: checking model availability");
    let response = ureq::get(&models_url)
        .set("Authorization", &format!("Bearer {}", model.api_key))
        .call()
        .map_err(|error| {
            format!(
                "failed to fetch models for profile '{profile_name}' from {models_url}: {error}"
            )
        })?;
    let body = response.into_json::<serde_json::Value>().map_err(|error| {
        format!("failed to parse models response for profile '{profile_name}' from {models_url}: {error}")
    })?;

    if model_response_contains(&body, &model.model_name) {
        Ok(())
    } else {
        Err(format!(
            "model profile '{profile_name}' configured model '{}' was not found at {models_url}",
            model.model_name
        ))
    }
}

pub fn model_response_contains(response: &serde_json::Value, model_name: &str) -> bool {
    response
        .get("data")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|models| {
            models.iter().any(|model| {
                model.get("id").and_then(serde_json::Value::as_str) == Some(model_name)
            })
        })
}
