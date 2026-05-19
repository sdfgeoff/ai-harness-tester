mod run;

use std::{path::PathBuf, time::Instant};
use clap::Parser;
use time::OffsetDateTime;

use orchestrator_core::config;
use orchestrator_core::models::{BatchRunReference, BatchSummary, RunStatus};
use orchestrator_core::test_selection::load_test_selection;
use orchestrator_core::util::{batch_id, format_timestamp, duration_ms};

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
        /// Comma-separated harness profile names from config.
        #[arg(long)]
        harnesses: String,
        /// Comma-separated model profile names from config.
        #[arg(long)]
        models: String,
        /// Config file path.
        #[arg(long, default_value = "config.json")]
        config: PathBuf,
        /// Comma-separated test folder names under tests/.
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
        CommandName::RunImage { harnesses, models, config, tests } => {
            execute_batch(&config, &parse_list("harnesses", &harnesses)?, &parse_list("models", &models)?, &parse_list("tests", &tests)?)
        }
    }
}

fn parse_list(kind: &str, raw: &str) -> Result<Vec<String>, String> {
    let values: Vec<String> = raw.split(',').map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned).collect();
    if values.is_empty() { return Err(format!("--{kind} must include at least one value")); }
    Ok(values)
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

    // Preflight
    let mut harness_image_ids = std::collections::BTreeMap::new();
    for harness in selected_harnesses {
        let profile = config.harnesses.get(harness).ok_or_else(|| format!("unknown harness profile '{harness}'"))?;
        let image_id = docker_runner::inspect_image(harness, &profile.image)?;
        harness_image_ids.insert(harness.clone(), image_id);
    }
    for model in selected_models {
        let profile = config.models.get(model).ok_or_else(|| format!("unknown model profile '{model}'"))?;
        config::preflight_model(model, profile)?;
    }
    for test in selected_tests {
        load_test_selection(test)?;
    }

    // Execute runs in test/model/harness order
    let mut failed_runs = 0usize;
    let mut run_references = Vec::new();

    for test in selected_tests {
        for model in selected_models {
            for harness in selected_harnesses {
                let harness_profile = config.harnesses.get(harness).ok_or_else(|| format!("unknown harness profile '{harness}'"))?;
                let model_profile = config.models.get(model).ok_or_else(|| format!("unknown model profile '{model}'"))?;

                let execution = run::execute_run(&batch_id, &config, harness, harness_profile, harness_image_ids.get(harness).expect("preflight"), model, model_profile, Some(test.as_str()))?;

                if execution.status != RunStatus::Completed { failed_runs += 1; }
                run_references.push(BatchRunReference { run_id: execution.run_id.clone(), results_path: format!("runs/{}/results.json", execution.run_id) });
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

    config::write_redacted_config_snapshot(&PathBuf::from(&config.results_dir).join(&batch_id), &config)?;

    if failed_runs > 0 {
        Err(format!("{failed_runs} run(s) failed"))
    } else {
        println!("batch {batch_id} completed successfully in {:.2?}", batch_started.elapsed());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use orchestrator_core::test_selection::validate_archive_path;
    use orchestrator_core::config::model_response_contains;
    use orchestrator_core::util::sanitize_fragment;
    use std::path::Path;

    #[test] fn clap_command_is_valid() { Cli::command().debug_assert(); }
    #[test] fn sanitizes_run_id_fragments() { assert_eq!(sanitize_fragment("harness-test/smoke:latest"), "harness-test-smoke-latest"); }
    #[test] fn rejects_parent_directory_archive_paths() { assert!(validate_archive_path(Path::new("../escape")).is_err()); }
    #[test] fn accepts_relative_archive_paths() { assert!(validate_archive_path(Path::new("src/message.txt")).is_ok()); }
    #[test] fn finds_model_in_openai_models_response() {
        let response = serde_json::json!({"object":"list","data":[{"id":"other-model"},{"id":"smoke-local"}]});
        assert!(model_response_contains(&response, "smoke-local"));
        assert!(!model_response_contains(&response, "missing"));
    }
}
