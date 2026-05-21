use std::{fs, path::PathBuf, time::Instant};
use time::OffsetDateTime;
use tokio::runtime::Runtime;

use crate::evaluation::{evaluate_completed_run, write_skipped_evaluation};
use orchestrator_core::{
    config::{Config, HarnessProfile, ModelProfile},
    models::{
        ResolvedHarness, ResolvedModel, RunArtifacts, RunError, RunExecution, RunInputs,
        RunMetrics, RunResolved, RunResult, RunSelection, RunStatus,
    },
    test_selection::{
        copy_prompt_artifact, extract_initial_state, load_test_selection, prepare_temp_prompt,
        remove_temp_prompt,
    },
    util::{duration_ms, format_timestamp, run_dir_name, run_id},
};

/// Intermediate state after successful setup, before harness execution.
struct SetupResult {
    working_dir: Option<PathBuf>,
    temp_prompt: Option<orchestrator_core::test_selection::TempPrompt>,
    runtime: Runtime,
    proxy_log_path: PathBuf,
    proxy: llm_proxy::ProxyHandle,
}

pub fn execute_run(
    batch_id: &str,
    config: &Config,
    harness_name: &str,
    harness: &HarnessProfile,
    harness_image_id: &str,
    model_name: &str,
    model: &ModelProfile,
    evaluator_image_id: Option<&str>,
    test: Option<&str>,
) -> Result<RunExecution, String> {
    let selected_test = test.map(load_test_selection).transpose()?;
    let started_at = OffsetDateTime::now_utc();
    let started = Instant::now();
    let test_name = selected_test
        .as_ref()
        .map(|test| test.name.as_str())
        .unwrap_or("no-test");
    let test_name_owned = test_name.to_string();
    let run_id = run_id(batch_id, harness_name, model_name, test_name);
    let run_dir_name = run_dir_name(harness_name, model_name, test_name);

    tracing::info!(run_id = %run_id, harness = harness_name, model = model_name, test = %test_name, "starting run");

    // Create directories
    let batch_dir = PathBuf::from(&config.results_dir).join(batch_id);
    fs::create_dir_all(&batch_dir).map_err(|error| {
        format!(
            "failed to create batch directory {}: {error}",
            batch_dir.display()
        )
    })?;
    orchestrator_core::config::write_redacted_config_snapshot(&batch_dir, config)?;

    let run_dir = batch_dir.join("runs").join(&run_dir_name);
    fs::create_dir_all(&run_dir).map_err(|error| {
        format!(
            "failed to create run directory {}: {error}",
            run_dir.display()
        )
    })?;
    let logs_dir = run_dir.join("logs");
    fs::create_dir_all(&logs_dir).map_err(|error| {
        format!(
            "failed to create logs directory {}: {error}",
            logs_dir.display()
        )
    })?;

    let harness_log_path = logs_dir.join("harness.log");

    // Setup phase: catch errors and write setup_failed results
    let setup = || -> Result<SetupResult, String> {
        // Working directory
        let working_dir = selected_test
            .as_ref()
            .map(|test| {
                let working_dir = run_dir.join("working_dir");
                extract_initial_state(&test.initial_state_path, &working_dir)?;
                Ok::<PathBuf, String>(working_dir)
            })
            .transpose()?;

        // Temp prompt
        let temp_prompt = selected_test
            .as_ref()
            .map(|test| prepare_temp_prompt(&run_id, &test.prompt_path))
            .transpose()?;

        // Start proxy
        let runtime =
            Runtime::new().map_err(|error| format!("failed to create async runtime: {error}"))?;
        let proxy_log_path = logs_dir.join("proxy.ndjson");
        let proxy = runtime.block_on(llm_proxy::start_proxy(llm_proxy::ProxyConfig {
            model_name: model.model_name.clone(),
            upstream_base_url: model.base_url.clone(),
            upstream_api_key: model.api_key.clone(),
            proxy_log_path: proxy_log_path.clone(),
        }))?;

        Ok(SetupResult {
            working_dir,
            temp_prompt,
            runtime,
            proxy_log_path,
            proxy,
        })
    }();

    let setup = match setup {
        Ok(s) => s,
        Err(error) => {
            tracing::error!(run_id = %run_id, error = %error, "setup failed");
            eprintln!("setup failed for run {}: {}", run_id, error);
            let duration = started.elapsed();
            let finished_at = OffsetDateTime::now_utc();
            let result = RunResult {
                run_id: run_id.clone(),
                batch_id: batch_id.to_owned(),
                status: RunStatus::SetupFailed,
                harness_exit_code: None,
                error: Some(RunError {
                    kind: "setup_failed".to_owned(),
                    message: error,
                }),
                started_at: format_timestamp(started_at)?,
                finished_at: format_timestamp(finished_at)?,
                duration_ms: duration_ms(duration),
                inputs: None,
                selection: RunSelection {
                    test: test_name_owned.clone(),
                    harness: harness_name.to_owned(),
                    model: model_name.to_owned(),
                },
                resolved: RunResolved {
                    harness: ResolvedHarness {
                        image: harness.image.clone(),
                        image_id: Some(harness_image_id.to_owned()),
                    },
                    model: ResolvedModel {
                        model_name: model.model_name.clone(),
                        base_url: model.base_url.clone(),
                    },
                },
                metrics: RunMetrics {
                    request_count: 0,
                    input_tokens: Some(0),
                    output_tokens: Some(0),
                    total_tokens: Some(0),
                    cache_read_tokens: Some(0),
                    cache_write_tokens: Some(0),
                },
                artifacts: RunArtifacts {
                    working_dir: None,
                    prompt: None,
                    harness_log: "logs/harness.log".to_owned(),
                    proxy_log: "logs/proxy.ndjson".to_owned(),
                },
            };
            orchestrator_core::models::write_results(&run_dir, &result)?;
            let evaluation_status = write_skipped_evaluation(&run_dir)?;
            return Ok(RunExecution {
                run_id,
                run_dir_name: run_dir_name.clone(),
                status: RunStatus::SetupFailed,
                evaluation_status,
            });
        }
    };

    let SetupResult {
        working_dir,
        temp_prompt,
        runtime,
        proxy_log_path,
        proxy,
    } = setup;

    println!(
        "running harness '{harness_name}' with image: {}",
        harness.image
    );

    let container_name = format!("harness-test-{run_id}");
    let mut mounts = Vec::new();
    let mut env = vec![
        ("LLM_URL".to_owned(), proxy.base_url.clone()),
        ("LLM_API_KEY".to_owned(), proxy.api_key.clone()),
    ];

    if let Some(working_dir) = working_dir.as_ref() {
        mounts.push(docker_runner::Mount {
            source: working_dir.clone(),
            target: "/workdir".to_owned(),
            read_only: false,
        });
        env.push(("WORKDIR".to_owned(), "/workdir".to_owned()));
    }

    if let Some(temp_prompt) = temp_prompt.as_ref() {
        mounts.push(docker_runner::Mount {
            source: temp_prompt.path.clone(),
            target: "/prompt/PROMPT.md".to_owned(),
            read_only: true,
        });
        env.push((
            "INITIAL_PROMPT_FILE".to_owned(),
            "/prompt/PROMPT.md".to_owned(),
        ));
    }

    let container_result = match docker_runner::run(&docker_runner::RunConfig {
        container_name: container_name.clone(),
        image: harness.image.clone(),
        network: docker_runner::NetworkMode::Host,
        mounts,
        workdir: working_dir.as_ref().map(|_| "/workdir".to_owned()),
        env,
        log_path: harness_log_path.clone(),
        console_prefix: Some(run_id.clone()),
        timeout: std::time::Duration::from_secs(config.timeout_seconds),
    }) {
        Ok(result) => result,
        Err(error) => {
            runtime.block_on(proxy.shutdown())?;
            // Docker spawn failure is a setup failure
            tracing::error!(run_id = %run_id, error = %error, "setup failed: docker runner");
            eprintln!(
                "setup failed for run {}: failed to start docker: {}",
                run_id, error
            );
            let duration = started.elapsed();
            let finished_at = OffsetDateTime::now_utc();
            let result = RunResult {
                run_id: run_id.clone(),
                batch_id: batch_id.to_owned(),
                status: RunStatus::SetupFailed,
                harness_exit_code: None,
                error: Some(RunError {
                    kind: "setup_failed".to_owned(),
                    message: format!("failed to start docker: {error}"),
                }),
                started_at: format_timestamp(started_at)?,
                finished_at: format_timestamp(finished_at)?,
                duration_ms: duration_ms(duration),
                inputs: None,
                selection: RunSelection {
                    test: test_name_owned.clone(),
                    harness: harness_name.to_owned(),
                    model: model_name.to_owned(),
                },
                resolved: RunResolved {
                    harness: ResolvedHarness {
                        image: harness.image.clone(),
                        image_id: Some(harness_image_id.to_owned()),
                    },
                    model: ResolvedModel {
                        model_name: model.model_name.clone(),
                        base_url: model.base_url.clone(),
                    },
                },
                metrics: RunMetrics {
                    request_count: 0,
                    input_tokens: Some(0),
                    output_tokens: Some(0),
                    total_tokens: Some(0),
                    cache_read_tokens: Some(0),
                    cache_write_tokens: Some(0),
                },
                artifacts: RunArtifacts {
                    working_dir: working_dir.map(|_| "working_dir".to_owned()),
                    prompt: None,
                    harness_log: "logs/harness.log".to_owned(),
                    proxy_log: "logs/proxy.ndjson".to_owned(),
                },
            };
            orchestrator_core::models::write_results(&run_dir, &result)?;
            let evaluation_status = write_skipped_evaluation(&run_dir)?;
            return Ok(RunExecution {
                run_id,
                run_dir_name: run_dir_name.clone(),
                status: RunStatus::SetupFailed,
                evaluation_status,
            });
        }
    };

    // Post-run cleanup
    let prompt_artifact = temp_prompt
        .as_ref()
        .map(|prompt| copy_prompt_artifact(prompt, &run_dir))
        .transpose()?;
    if let Some(prompt) = temp_prompt.as_ref() {
        remove_temp_prompt(prompt)?;
    }
    runtime.block_on(proxy.shutdown())?;

    // Aggregate metrics from proxy log
    let metrics = RunMetrics::from_proxy_log(&proxy_log_path)?;

    // Build and write results
    let duration = started.elapsed();
    let finished_at = OffsetDateTime::now_utc();
    let (run_status, harness_exit_code, error) = match container_result.status {
        docker_runner::ContainerStatus::TimedOut => (
            RunStatus::TimedOut,
            None,
            Some(RunError {
                kind: "timeout".to_owned(),
                message: format!("Run exceeded timeout of {} seconds", config.timeout_seconds),
            }),
        ),
        docker_runner::ContainerStatus::Completed => (RunStatus::Completed, Some(0), None),
        docker_runner::ContainerStatus::Failed => {
            let harness_exit_code = container_result.exit_code;
            (
                RunStatus::Failed,
                harness_exit_code,
                Some(RunError {
                    kind: "failed".to_owned(),
                    message: format!("Harness exited with code {:?}", harness_exit_code),
                }),
            )
        }
    };

    let result = RunResult {
        run_id: run_id.clone(),
        batch_id: batch_id.to_owned(),
        status: run_status,
        harness_exit_code,
        error,
        started_at: format_timestamp(started_at)?,
        finished_at: format_timestamp(finished_at)?,
        duration_ms: duration_ms(duration),
        inputs: selected_test.as_ref().map(|test| RunInputs {
            initial_state_sha256: test.initial_state_sha256.clone(),
            prompt_sha256: test.prompt_sha256.clone(),
        }),
        selection: RunSelection {
            test: test_name_owned.clone(),
            harness: harness_name.to_owned(),
            model: model_name.to_owned(),
        },
        resolved: RunResolved {
            harness: ResolvedHarness {
                image: harness.image.clone(),
                image_id: Some(harness_image_id.to_owned()),
            },
            model: ResolvedModel {
                model_name: model.model_name.clone(),
                base_url: model.base_url.clone(),
            },
        },
        metrics,
        artifacts: RunArtifacts {
            working_dir: working_dir.map(|_| "working_dir".to_owned()),
            prompt: prompt_artifact,
            harness_log: "logs/harness.log".to_owned(),
            proxy_log: "logs/proxy.ndjson".to_owned(),
        },
    };

    orchestrator_core::models::write_results(&run_dir, &result)?;
    let evaluation_status = match run_status {
        RunStatus::Completed => {
            let selected_test = selected_test
                .as_ref()
                .ok_or_else(|| "completed run missing selected test for evaluation".to_owned())?;
            let evaluator_image_id = evaluator_image_id.ok_or_else(|| {
                format!(
                    "completed run '{}' missing resolved evaluator image id",
                    selected_test.name
                )
            })?;
            evaluate_completed_run(config, &run_id, &run_dir, selected_test, evaluator_image_id)?
        }
        _ => write_skipped_evaluation(&run_dir)?,
    };
    println!("wrote {}", run_dir.join("results.json").display());

    match run_status {
        RunStatus::Completed => {
            println!("container completed successfully in {:.2?}", duration);
            Ok(RunExecution {
                run_id,
                run_dir_name: run_dir_name.clone(),
                status: RunStatus::Completed,
                evaluation_status,
            })
        }
        RunStatus::TimedOut => {
            tracing::warn!(run_id = %run_id, duration_ms = duration_ms(duration), "container timed out");
            eprintln!("container timed out after {:.2?}", duration);
            Ok(RunExecution {
                run_id,
                run_dir_name: run_dir_name.clone(),
                status: RunStatus::TimedOut,
                evaluation_status,
            })
        }
        RunStatus::Failed => {
            tracing::error!(run_id = %run_id, harness_exit_code = ?harness_exit_code, duration_ms = duration_ms(duration), "container failed");
            eprintln!("container failed after {:.2?}", duration);
            Ok(RunExecution {
                run_id,
                run_dir_name: run_dir_name.clone(),
                status: RunStatus::Failed,
                evaluation_status,
            })
        }
        RunStatus::SetupFailed => {
            tracing::error!(run_id = %run_id, duration_ms = duration_ms(duration), "setup failed");
            eprintln!("setup failed after {:.2?}", duration);
            Ok(RunExecution {
                run_id,
                run_dir_name: run_dir_name.clone(),
                status: RunStatus::SetupFailed,
                evaluation_status,
            })
        }
    }
}
