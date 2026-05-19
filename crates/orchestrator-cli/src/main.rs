mod config;
mod models;
mod output;
mod run;
mod test_selection;
mod util;

use std::{path::PathBuf, time::Instant};
use clap::Parser;
use time::OffsetDateTime;

use config::{inspect_docker_image, load_config, preflight_model};
use models::{BatchRunReference, BatchSummary, RunStatus};
use test_selection::load_test_selection;
use util::{batch_id, format_timestamp, duration_ms};

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
    match run(Cli::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        CommandName::RunImage {
            harnesses,
            models,
            config: config_path,
            tests,
        } => execute_batch(
            &config_path,
            &parse_selection_list("harnesses", &harnesses)?,
            &parse_selection_list("models", &models)?,
            &parse_selection_list("tests", &tests)?,
        ),
    }
}

fn parse_selection_list(kind: &str, raw: &str) -> Result<Vec<String>, String> {
    let values = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    if values.is_empty() {
        return Err(format!("--{kind} must include at least one value"));
    }

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
    let config = load_config(config_path)?;

    // Preflight validation
    for harness in selected_harnesses {
        let Some(profile) = config.harnesses.get(harness) else {
            return Err(format!("unknown harness profile '{harness}'"));
        };
        inspect_docker_image(harness, &profile.image)?;
    }
    for model in selected_models {
        let Some(profile) = config.models.get(model) else {
            return Err(format!("unknown model profile '{model}'"));
        };
        preflight_model(model, profile)?;
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
                    model,
                    model_profile,
                    Some(test.as_str()),
                )?;

                if execution.status != RunStatus::Completed {
                    failed_runs += 1;
                }
                run_references.push(BatchRunReference {
                    run_id: execution.run_id.clone(),
                    results_path: format!("runs/{}/results.json", execution.run_id),
                });
            }
        }
    }

    // Write batch summary
    let finished_at = OffsetDateTime::now_utc();
    let batch_duration = batch_started.elapsed();
    models::write_batch_summary(
        &std::path::PathBuf::from("results").join(&batch_id),
        &BatchSummary {
            batch_id: batch_id.clone(),
            started_at: format_timestamp(batch_started_at)?,
            finished_at: format_timestamp(finished_at)?,
            duration_ms: duration_ms(batch_duration),
            config_path: config_path.display().to_string(),
            runs: run_references,
        },
    )?;

    if failed_runs > 0 {
        Err(format!("{failed_runs} run(s) failed"))
    } else {
        println!(
            "batch {batch_id} completed successfully in {:.2?}",
            batch_duration
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use crate::test_selection::validate_archive_path;
    use crate::config::model_response_contains;
    use crate::util::sanitize_fragment;
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
        let response = serde_json::json!({
            "object": "list",
            "data": [
                {"id": "other-model"},
                {"id": "smoke-local"}
            ]
        });

        assert!(model_response_contains(&response, "smoke-local"));
        assert!(!model_response_contains(&response, "missing"));
    }
}
