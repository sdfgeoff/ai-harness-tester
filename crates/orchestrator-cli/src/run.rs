use std::{
    fs::{self, File},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Instant,
};
use time::OffsetDateTime;
use tokio::runtime::Runtime;

use orchestrator_core::{
    config::{Config, HarnessProfile, ModelProfile},
    models::{
        ResolvedHarness, ResolvedModel, RunArtifacts, RunError, RunExecution, RunInputs,
        RunMetrics, RunResult, RunResolved, RunSelection, RunStatus,
    },
    output::{copy_output, join_log_thread},
    test_selection::{
        copy_prompt_artifact, load_test_selection, prepare_temp_prompt, remove_temp_prompt,
        extract_initial_state,
    },
    util::{format_timestamp, duration_ms, run_id},
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

    let run_dir = batch_dir.join("runs").join(&run_id);
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

    // Harness log
    let harness_log_path = logs_dir.join("harness.log");
    let harness_log = Arc::new(Mutex::new(File::create(&harness_log_path).map_err(
        |error| {
            format!(
                "failed to create harness log {}: {error}",
                harness_log_path.display()
            )
        },
    )?));

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
        let runtime = Runtime::new()
            .map_err(|error| format!("failed to create async runtime: {error}"))?;
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
            return Ok(RunExecution {
                run_id,
                status: RunStatus::SetupFailed,
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

    // Build and run Docker command
    let container_name = format!("harness-test-{run_id}");
    let mut command = Command::new("docker");
    command.arg("run").arg("--rm").arg("--name").arg(&container_name).arg("--network").arg("host");

    if let Some(working_dir) = working_dir.as_ref() {
        let mount_source = fs::canonicalize(working_dir).map_err(|error| {
            format!(
                "failed to canonicalize working directory {}: {error}",
                working_dir.display()
            )
        })?;
        command
            .arg("--volume")
            .arg(format!("{}:/workdir", mount_source.display()))
            .arg("--workdir")
            .arg("/workdir")
            .arg("--env")
            .arg("WORKDIR=/workdir");
    }

    command
        .arg("--env")
        .arg(format!("LLM_URL={}", proxy.base_url))
        .arg("--env")
        .arg(format!("LLM_API_KEY={}", proxy.api_key));

    if let Some(temp_prompt) = temp_prompt.as_ref() {
        let mount_source = fs::canonicalize(&temp_prompt.path).map_err(|error| {
            format!(
                "failed to canonicalize temporary prompt {}: {error}",
                temp_prompt.path.display()
            )
        })?;
        command
            .arg("--volume")
            .arg(format!("{}:/prompt/PROMPT.md:ro", mount_source.display()))
            .arg("--env")
            .arg("INITIAL_PROMPT_FILE=/prompt/PROMPT.md");
    }

    let mut child = match command
        .arg(&harness.image)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            runtime.block_on(proxy.shutdown())?;
            // Docker spawn failure is a setup failure
            tracing::error!(run_id = %run_id, error = %error, "setup failed: docker spawn");
            eprintln!("setup failed for run {}: failed to start docker: {}", run_id, error);
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
            return Ok(RunExecution {
                run_id,
                status: RunStatus::SetupFailed,
            });
        }
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture docker stdout".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture docker stderr".to_owned())?;

    let stdout_thread = copy_output(stdout, Arc::clone(&harness_log), run_id.clone());
    let stderr_thread = copy_output(stderr, Arc::clone(&harness_log), run_id.clone());

    // Wait for container with timeout
    let timeout_instant = std::time::Instant::now() + std::time::Duration::from_secs(config.timeout_seconds);
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(s)) => break (s, false),
            Ok(None) => {
                if std::time::Instant::now() >= timeout_instant {
                    tracing::warn!(run_id = %run_id, timeout_seconds = config.timeout_seconds, "run exceeded timeout, killing container");
                    eprintln!(
                        "run {} exceeded timeout of {} seconds, killing container",
                        run_id, config.timeout_seconds
                    );
                    // Kill the container
                    let _ = Command::new("docker")
                        .arg("kill")
                        .arg(format!("harness-test-{run_id}"))
                        .output();
                    let _ = child.kill();
                    break (child.wait().unwrap_or_default(), true);
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(error) => {
                return Err(format!("failed to check docker status: {error}"));
            }
        }
    };

    join_log_thread(stdout_thread)?;
    join_log_thread(stderr_thread)?;

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
    let (run_status, harness_exit_code, error) = if timed_out {
        (RunStatus::TimedOut, None, Some(RunError {
            kind: "timeout".to_owned(),
            message: format!("Run exceeded timeout of {} seconds", config.timeout_seconds),
        }))
    } else {
        let harness_exit_code = status.code();
        let run_status = match harness_exit_code {
            Some(0) => RunStatus::Completed,
            _ => RunStatus::Failed,
        };
        let error = if run_status == RunStatus::Failed {
            Some(RunError {
                kind: "failed".to_owned(),
                message: format!("Harness exited with code {:?}", harness_exit_code),
            })
        } else {
            None
        };
        (run_status, harness_exit_code, error)
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
        inputs: selected_test.map(|test| RunInputs {
            initial_state_sha256: test.initial_state_sha256,
            prompt_sha256: test.prompt_sha256,
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
    println!("wrote {}", run_dir.join("results.json").display());

    match run_status {
        RunStatus::Completed => {
            println!("container completed successfully in {:.2?}", duration);
            Ok(RunExecution {
                run_id,
                status: RunStatus::Completed,
            })
        }
        RunStatus::TimedOut => {
            tracing::warn!(run_id = %run_id, duration_ms = duration_ms(duration), "container timed out");
            eprintln!("container timed out after {:.2?}", duration);
            Ok(RunExecution {
                run_id,
                status: RunStatus::TimedOut,
            })
        }
        RunStatus::Failed => {
            tracing::error!(run_id = %run_id, harness_exit_code = ?harness_exit_code, duration_ms = duration_ms(duration), "container failed");
            eprintln!("container failed after {:.2?}", duration);
            Ok(RunExecution {
                run_id,
                status: RunStatus::Failed,
            })
        }
        RunStatus::SetupFailed => {
            tracing::error!(run_id = %run_id, duration_ms = duration_ms(duration), "setup failed");
            eprintln!("setup failed after {:.2?}", duration);
            Ok(RunExecution {
                run_id,
                status: RunStatus::SetupFailed,
            })
        }
    }
}
