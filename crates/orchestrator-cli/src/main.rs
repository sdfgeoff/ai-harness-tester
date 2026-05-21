mod evaluation;
mod run;

use clap::Parser;
use std::{path::PathBuf, time::Instant};
use time::OffsetDateTime;

use orchestrator_core::config;
use orchestrator_core::models::{BatchRunReference, BatchSummary, RunStatus};
use orchestrator_core::test_selection::{evaluator_image_tag, load_test_selection};
use orchestrator_core::util::{batch_id, duration_ms, format_timestamp};

#[derive(Debug, Parser)]
#[command(
    name = "orchestrator",
    version,
    about = "Run coding harness benchmark workflows"
)]
struct Cli {
    #[command(subcommand)]
    command: CommandName,
}

#[derive(Debug, clap::Subcommand)]
enum CommandName {
    /// Run a Docker image once and report its exit status.
    RunImage {
        /// Comma-separated harness profile names from config (use "all" for all non-smoke harnesses).
        #[arg(long)]
        harnesses: String,
        /// Comma-separated model profile names from config (use "all" for all models).
        #[arg(long)]
        models: String,
        /// Config file path.
        #[arg(long, default_value = "config.json")]
        config: PathBuf,
        /// Comma-separated test folder names under tests/ (use "all" for all tests).
        #[arg(long)]
        tests: String,
    },
}

fn main() -> std::process::ExitCode {
    init_tracing();
    match run_cmd(Cli::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %error);
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .pretty()
        .try_init();
}

fn run_cmd(cli: Cli) -> Result<(), String> {
    match cli.command {
        CommandName::RunImage {
            harnesses,
            models,
            config,
            tests,
        } => execute_batch(
            &config,
            &parse_list("harnesses", &harnesses)?,
            &parse_list("models", &models)?,
            &parse_list("tests", &tests)?,
        ),
    }
}

fn parse_list(kind: &str, raw: &str) -> Result<Vec<String>, String> {
    let values: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect();
    if values.is_empty() {
        return Err(format!("--{kind} must include at least one value"));
    }
    Ok(values)
}

/// Expand "all" meta-value for harnesses (all except "smoke").
fn expand_all_harnesses(
    selected: &[String],
    config: &config::Config,
) -> Result<Vec<String>, String> {
    if selected.iter().any(|s| s == "all") {
        Ok(config
            .harnesses
            .keys()
            .filter(|h| *h != "smoke")
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .collect())
    } else {
        Ok(selected.to_vec())
    }
}

/// Expand "all" meta-value for models.
fn expand_all_models(selected: &[String], config: &config::Config) -> Result<Vec<String>, String> {
    if selected.iter().any(|s| s == "all") {
        Ok(config.models.keys().cloned().collect())
    } else {
        Ok(selected.to_vec())
    }
}

/// Expand "all" meta-value for tests (all folders under tests/).
fn expand_all_tests(selected: &[String]) -> Result<Vec<String>, String> {
    if selected.iter().any(|s| s == "all") {
        let tests_dir = std::path::Path::new("tests");
        if !tests_dir.is_dir() {
            return Err("tests/ directory not found".to_owned());
        }
        let mut tests: Vec<String> = std::fs::read_dir(tests_dir)
            .map_err(|e| format!("failed to read tests/: {e}"))?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                if entry.path().is_dir() {
                    entry.file_name().into_string().ok()
                } else {
                    None
                }
            })
            .collect();
        tests.sort();
        Ok(tests)
    } else {
        Ok(selected.to_vec())
    }
}

fn execute_batch(
    config_path: &PathBuf,
    selected_harnesses: &[String],
    selected_models: &[String],
    selected_tests: &[String],
) -> Result<(), String> {
    let batch_started_at = OffsetDateTime::now_utc();
    let batch_started = Instant::now();
    let batch_id = batch_id(batch_started_at)?;
    let config = config::load_config(config_path)?;

    // Expand "all" meta-values
    let harnesses = expand_all_harnesses(selected_harnesses, &config)?;
    let models = expand_all_models(selected_models, &config)?;
    let tests = expand_all_tests(selected_tests)?;

    // Preflight
    let mut harness_image_ids = std::collections::BTreeMap::new();
    let mut evaluator_image_ids = std::collections::BTreeMap::new();
    for harness in &harnesses {
        let profile = config
            .harnesses
            .get(harness)
            .ok_or_else(|| format!("unknown harness profile '{harness}'"))?;
        let image_id = docker_runner::inspect_image(harness, &profile.image)?;
        harness_image_ids.insert(harness.clone(), image_id);
    }
    for model in &models {
        let profile = config
            .models
            .get(model)
            .ok_or_else(|| format!("unknown model profile '{model}'"))?;
        config::preflight_model(model, profile)?;
    }
    for test in &tests {
        let selection = load_test_selection(test)?;
        let evaluator_image = evaluator_image_tag(test);
        let evaluator_image_id = docker_runner::inspect_image(test, &evaluator_image)?;
        evaluator_image_ids.insert(selection.name, evaluator_image_id);
    }

    // Execute runs in test/model/harness order
    let mut failed_runs = 0usize;
    let mut run_references = Vec::new();

    for test in &tests {
        for model in &models {
            for harness in &harnesses {
                let harness_profile = config
                    .harnesses
                    .get(harness)
                    .ok_or_else(|| format!("unknown harness profile '{harness}'"))?;
                let model_profile = config
                    .models
                    .get(model)
                    .ok_or_else(|| format!("unknown model profile '{model}'"))?;

                let execution = run::execute_run(
                    &batch_id,
                    &config,
                    harness,
                    harness_profile,
                    harness_image_ids.get(harness).expect("preflight"),
                    model,
                    model_profile,
                    evaluator_image_ids.get(test).map(String::as_str),
                    Some(test.as_str()),
                )?;

                if execution.status != RunStatus::Completed {
                    failed_runs += 1;
                }
                run_references.push(BatchRunReference {
                    run_id: execution.run_id.clone(),
                    results_path: format!("runs/{}/results.json", execution.run_dir_name),
                    evaluation_path: format!("runs/{}/evaluation.json", execution.run_dir_name),
                });
            }
        }
    }

    // Write batch summary
    let finished_at = OffsetDateTime::now_utc();
    orchestrator_core::models::write_batch_summary(
        &PathBuf::from(&config.results_dir).join(&batch_id),
        &BatchSummary {
            batch_id: batch_id.clone(),
            started_at: format_timestamp(batch_started_at)?,
            finished_at: format_timestamp(finished_at)?,
            duration_ms: duration_ms(batch_started.elapsed()),
            config_path: config_path.display().to_string(),
            runs: run_references,
        },
    )?;

    config::write_redacted_config_snapshot(
        &PathBuf::from(&config.results_dir).join(&batch_id),
        &config,
    )?;

    if failed_runs > 0 {
        Err(format!("{failed_runs} run(s) failed"))
    } else {
        println!(
            "batch {batch_id} completed successfully in {:.2?}",
            batch_started.elapsed()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use orchestrator_core::config::model_response_contains;
    use orchestrator_core::test_selection::validate_archive_path;
    use orchestrator_core::util::sanitize_fragment;
    use std::path::Path;

    #[test]
    fn clap_command_is_valid() {
        Cli::command().debug_assert();
    }
    #[test]
    fn sanitizes_run_id_fragments() {
        assert_eq!(
            sanitize_fragment("harness-test/smoke:latest"),
            "harness-test-smoke-latest"
        );
    }
    #[test]
    fn rejects_parent_directory_archive_paths() {
        assert!(validate_archive_path(Path::new("../escape")).is_err());
    }
    #[test]
    fn accepts_relative_archive_paths() {
        assert!(validate_archive_path(Path::new("src/message.txt")).is_ok());
    }
    #[test]
    fn finds_model_in_openai_models_response() {
        let response =
            serde_json::json!({"object":"list","data":[{"id":"other-model"},{"id":"smoke-local"}]});
        assert!(model_response_contains(&response, "smoke-local"));
        assert!(!model_response_contains(&response, "missing"));
    }

    #[test]
    fn expands_all_harnesses_excludes_smoke() {
        let config = config::Config {
            models: std::collections::BTreeMap::new(),
            harnesses: ["smoke", "pi", "claudecode"]
                .into_iter()
                .map(|h| {
                    (
                        h.to_owned(),
                        orchestrator_core::config::HarnessProfile {
                            image: "test".to_owned(),
                        },
                    )
                })
                .collect(),
            results_dir: "results".to_owned(),
            timeout_seconds: 300,
            evaluation_timeout_seconds: 60,
        };
        let result = expand_all_harnesses(&["all".to_owned()], &config).unwrap();
        assert_eq!(result, vec!["claudecode", "pi"]);
    }

    #[test]
    fn passes_through_explicit_harnesses() {
        let config = config::Config {
            models: std::collections::BTreeMap::new(),
            harnesses: ["smoke", "pi", "claudecode"]
                .into_iter()
                .map(|h| {
                    (
                        h.to_owned(),
                        orchestrator_core::config::HarnessProfile {
                            image: "test".to_owned(),
                        },
                    )
                })
                .collect(),
            results_dir: "results".to_owned(),
            timeout_seconds: 300,
            evaluation_timeout_seconds: 60,
        };
        let result = expand_all_harnesses(&["smoke".to_owned(), "pi".to_owned()], &config).unwrap();
        assert_eq!(result, vec!["smoke", "pi"]);
    }

    #[test]
    fn expands_all_models() {
        let config = config::Config {
            models: ["a", "b"]
                .into_iter()
                .map(|m| {
                    (
                        m.to_owned(),
                        orchestrator_core::config::ModelProfile {
                            model_name: m.to_owned(),
                            base_url: "http://test".to_owned(),
                            api_key: "".to_owned(),
                        },
                    )
                })
                .collect(),
            harnesses: std::collections::BTreeMap::new(),
            results_dir: "results".to_owned(),
            timeout_seconds: 300,
            evaluation_timeout_seconds: 60,
        };
        let result = expand_all_models(&["all".to_owned()], &config).unwrap();
        assert_eq!(result, vec!["a", "b"]);
    }
}
