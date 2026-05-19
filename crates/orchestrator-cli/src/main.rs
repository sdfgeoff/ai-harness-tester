use clap::Parser;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use time::{format_description::FormatItem, macros::format_description, OffsetDateTime};

const RUN_ID_TIME_FORMAT: &[FormatItem<'_>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");

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

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        CommandName::RunImage {
            harnesses,
            models,
            config,
            tests,
        } => {
            let batch_started_at = OffsetDateTime::now_utc();
            let batch_started = Instant::now();
            let batch_id = batch_id(batch_started_at)?;
            let config_path = config.clone();
            let config = load_config(&config)?;
            let selected_harnesses = parse_selection_list("harnesses", &harnesses)?;
            let selected_models = parse_selection_list("models", &models)?;
            let selected_tests = parse_selection_list("tests", &tests)?;

            for harness in &selected_harnesses {
                let Some(profile) = config.harnesses.get(harness) else {
                    return Err(format!("unknown harness profile '{harness}'"));
                };
                inspect_docker_image(harness, &profile.image)?;
            }
            for model in &selected_models {
                let Some(profile) = config.models.get(model) else {
                    return Err(format!("unknown model profile '{model}'"));
                };
                preflight_model(model, profile)?;
            }
            for test in &selected_tests {
                load_test_selection(test)?;
            }

            let mut failed_runs = 0usize;
            let mut run_references = Vec::new();
            for test in &selected_tests {
                for model in &selected_models {
                    for harness in &selected_harnesses {
                        let harness_profile = config
                            .harnesses
                            .get(harness)
                            .ok_or_else(|| format!("unknown harness profile '{harness}'"))?;
                        let model_profile = config
                            .models
                            .get(model)
                            .ok_or_else(|| format!("unknown model profile '{model}'"))?;
                        let execution = run_image(
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

            let finished_at = OffsetDateTime::now_utc();
            let batch_duration = batch_started.elapsed();
            write_batch_summary(
                &PathBuf::from("results").join(&batch_id),
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

fn inspect_docker_image(harness_name: &str, image: &str) -> Result<(), String> {
    let output = Command::new("docker")
        .arg("image")
        .arg("inspect")
        .arg(image)
        .output()
        .map_err(|error| {
            format!("failed to inspect Docker image for harness '{harness_name}': {error}")
        })?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Docker image for harness '{harness_name}' does not exist locally: {image}: {}",
            stderr.trim()
        ))
    }
}

fn preflight_model(profile_name: &str, model: &ModelProfile) -> Result<(), String> {
    let models_url = format!("{}/models", model.base_url.trim_end_matches('/'));
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

fn model_response_contains(response: &serde_json::Value, model_name: &str) -> bool {
    response
        .get("data")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|models| {
            models.iter().any(|model| {
                model.get("id").and_then(serde_json::Value::as_str) == Some(model_name)
            })
        })
}

fn run_image(
    batch_id: &str,
    config: &Config,
    harness_name: &str,
    harness: &HarnessProfile,
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
    let run_id = run_id(batch_id, harness_name, model_name, test_name);
    let batch_dir = PathBuf::from("results").join(batch_id);
    fs::create_dir_all(&batch_dir).map_err(|error| {
        format!(
            "failed to create batch directory {}: {error}",
            batch_dir.display()
        )
    })?;
    write_redacted_config_snapshot(&batch_dir, config)?;
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
    let harness_log_path = logs_dir.join("harness.log");
    let harness_log = Arc::new(Mutex::new(File::create(&harness_log_path).map_err(
        |error| {
            format!(
                "failed to create harness log {}: {error}",
                harness_log_path.display()
            )
        },
    )?));
    let working_dir = selected_test
        .as_ref()
        .map(|test| {
            let working_dir = run_dir.join("working_dir");
            extract_initial_state(&test.initial_state_path, &working_dir)?;
            Ok::<PathBuf, String>(working_dir)
        })
        .transpose()?;
    let temp_prompt = selected_test
        .as_ref()
        .map(|test| prepare_temp_prompt(&run_id, &test.prompt_path))
        .transpose()?;
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| format!("failed to create async runtime: {error}"))?;
    let proxy = runtime.block_on(llm_proxy::start_proxy(llm_proxy::ProxyConfig {
        model_name: model.model_name.clone(),
        upstream_base_url: model.base_url.clone(),
        upstream_api_key: model.api_key.clone(),
    }))?;

    println!(
        "running harness '{harness_name}' with image: {}",
        harness.image
    );

    let mut command = Command::new("docker");
    command.arg("run").arg("--rm");
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
            return Err(format!("failed to start docker: {error}"));
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
    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for docker: {error}"))?;
    join_log_thread(stdout_thread)?;
    join_log_thread(stderr_thread)?;

    let prompt_artifact = temp_prompt
        .as_ref()
        .map(|prompt| copy_prompt_artifact(prompt, &run_dir))
        .transpose()?;
    if let Some(prompt) = temp_prompt.as_ref() {
        remove_temp_prompt(prompt)?;
    }
    runtime.block_on(proxy.shutdown())?;

    let duration = started.elapsed();
    let finished_at = OffsetDateTime::now_utc();
    let harness_exit_code = status.code();
    let run_status = match harness_exit_code {
        Some(0) => RunStatus::Completed,
        _ => RunStatus::Failed,
    };
    let result = RunResult {
        run_id: run_id.clone(),
        batch_id: batch_id.to_owned(),
        status: run_status,
        harness_exit_code,
        started_at: format_timestamp(started_at)?,
        finished_at: format_timestamp(finished_at)?,
        duration_ms: duration_ms(duration),
        inputs: selected_test.map(|test| RunInputs {
            test: test.name,
            initial_state_sha256: test.initial_state_sha256,
            prompt_sha256: test.prompt_sha256,
        }),
        selection: RunSelection {
            harness: harness_name.to_owned(),
            model: model_name.to_owned(),
        },
        resolved: RunResolved {
            harness: ResolvedHarness {
                image: harness.image.clone(),
            },
            model: ResolvedModel {
                model_name: model.model_name.clone(),
                base_url: model.base_url.clone(),
            },
        },
        artifacts: RunArtifacts {
            working_dir: working_dir.map(|_| "working_dir".to_owned()),
            prompt: prompt_artifact,
            harness_log: "logs/harness.log".to_owned(),
        },
    };
    write_results(&run_dir, &result)?;
    println!("wrote {}", run_dir.join("results.json").display());

    match status.code() {
        Some(0) => {
            println!("container completed successfully in {:.2?}", duration);
            Ok(RunExecution {
                run_id,
                status: RunStatus::Completed,
            })
        }
        Some(code) => {
            eprintln!("container exited with status {code} after {:.2?}", duration);
            Ok(RunExecution {
                run_id,
                status: RunStatus::Failed,
            })
        }
        None => {
            eprintln!(
                "container terminated without an exit code after {:.2?}",
                duration
            );
            Ok(RunExecution {
                run_id,
                status: RunStatus::Failed,
            })
        }
    }
}

#[derive(Debug)]
struct RunExecution {
    run_id: String,
    status: RunStatus,
}

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    models: BTreeMap<String, ModelProfile>,
    harnesses: BTreeMap<String, HarnessProfile>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ModelProfile {
    model_name: String,
    base_url: String,
    api_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct HarnessProfile {
    image: String,
}

fn load_config(path: &Path) -> Result<Config, String> {
    let file = File::open(path)
        .map_err(|error| format!("failed to open config {}: {error}", path.display()))?;
    serde_json::from_reader(file)
        .map_err(|error| format!("failed to parse config {}: {error}", path.display()))
}

#[derive(Debug, Serialize)]
struct RedactedConfig<'a> {
    models: BTreeMap<&'a str, RedactedModelProfile<'a>>,
    harnesses: &'a BTreeMap<String, HarnessProfile>,
}

#[derive(Debug, Serialize)]
struct RedactedModelProfile<'a> {
    model_name: &'a str,
    base_url: &'a str,
    api_key: &'static str,
}

fn write_redacted_config_snapshot(batch_dir: &Path, config: &Config) -> Result<(), String> {
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

#[derive(Debug, Serialize)]
struct BatchSummary {
    batch_id: String,
    started_at: String,
    finished_at: String,
    duration_ms: u64,
    config_path: String,
    runs: Vec<BatchRunReference>,
}

#[derive(Debug, Serialize)]
struct BatchRunReference {
    run_id: String,
    results_path: String,
}

fn write_batch_summary(batch_dir: &Path, summary: &BatchSummary) -> Result<(), String> {
    let path = batch_dir.join("summary.json");
    let file = File::create(&path)
        .map_err(|error| format!("failed to create summary file {}: {error}", path.display()))?;
    serde_json::to_writer_pretty(file, summary)
        .map_err(|error| format!("failed to write summary file {}: {error}", path.display()))
}

#[derive(Debug)]
struct TestSelection {
    name: String,
    initial_state_path: PathBuf,
    prompt_path: PathBuf,
    initial_state_sha256: String,
    prompt_sha256: String,
}

fn load_test_selection(name: &str) -> Result<TestSelection, String> {
    let test_dir = PathBuf::from("tests").join(name);
    if !test_dir.is_dir() {
        return Err(format!(
            "test '{name}' does not exist at {}",
            test_dir.display()
        ));
    }

    let initial_state = test_dir.join("initial_state.zip");
    if !initial_state.is_file() {
        return Err(format!(
            "test '{name}' is missing required file {}",
            initial_state.display()
        ));
    }

    let prompt = test_dir.join("PROMPT.md");
    if !prompt.is_file() {
        return Err(format!(
            "test '{name}' is missing required file {}",
            prompt.display()
        ));
    }

    Ok(TestSelection {
        name: name.to_owned(),
        initial_state_path: initial_state.clone(),
        prompt_path: prompt.clone(),
        initial_state_sha256: sha256_file(&initial_state)?,
        prompt_sha256: sha256_file(&prompt)?,
    })
}

#[derive(Debug)]
struct TempPrompt {
    path: PathBuf,
}

fn prepare_temp_prompt(run_id: &str, prompt_path: &Path) -> Result<TempPrompt, String> {
    let temp_dir = std::env::temp_dir().join("harness-test-prompts");
    fs::create_dir_all(&temp_dir).map_err(|error| {
        format!(
            "failed to create temporary prompt directory {}: {error}",
            temp_dir.display()
        )
    })?;

    let path = temp_dir.join(format!("{run_id}-PROMPT.md"));
    fs::copy(prompt_path, &path).map_err(|error| {
        format!(
            "failed to copy prompt {} to temporary prompt {}: {error}",
            prompt_path.display(),
            path.display()
        )
    })?;

    Ok(TempPrompt { path })
}

fn copy_prompt_artifact(prompt: &TempPrompt, run_dir: &Path) -> Result<String, String> {
    let artifact_path = run_dir.join("PROMPT.md");
    fs::copy(&prompt.path, &artifact_path).map_err(|error| {
        format!(
            "failed to copy temporary prompt {} to artifact {}: {error}",
            prompt.path.display(),
            artifact_path.display()
        )
    })?;

    Ok("PROMPT.md".to_owned())
}

fn remove_temp_prompt(prompt: &TempPrompt) -> Result<(), String> {
    fs::remove_file(&prompt.path).map_err(|error| {
        format!(
            "failed to remove temporary prompt {}: {error}",
            prompt.path.display()
        )
    })
}

fn extract_initial_state(zip_path: &Path, working_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(working_dir).map_err(|error| {
        format!(
            "failed to create working directory {}: {error}",
            working_dir.display()
        )
    })?;

    let file = File::open(zip_path)
        .map_err(|error| format!("failed to open {}: {error}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("failed to read zip {}: {error}", zip_path.display()))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("failed to read zip entry {index}: {error}"))?;
        let enclosed_name = entry
            .enclosed_name()
            .ok_or_else(|| format!("unsafe zip entry path: {}", entry.name()))?
            .to_owned();

        validate_archive_path(&enclosed_name)?;
        reject_symlink_entry(&entry)?;

        let output_path = working_dir.join(&enclosed_name);
        if entry.is_dir() {
            fs::create_dir_all(&output_path).map_err(|error| {
                format!(
                    "failed to create extracted directory {}: {error}",
                    output_path.display()
                )
            })?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create extracted parent directory {}: {error}",
                    parent.display()
                )
            })?;
        }

        let mut output = File::create(&output_path).map_err(|error| {
            format!(
                "failed to create extracted file {}: {error}",
                output_path.display()
            )
        })?;
        io::copy(&mut entry, &mut output).map_err(|error| {
            format!(
                "failed to extract {} to {}: {error}",
                entry.name(),
                output_path.display()
            )
        })?;
    }

    Ok(())
}

fn validate_archive_path(path: &Path) -> Result<(), String> {
    if path.is_absolute() {
        return Err(format!(
            "unsafe absolute zip entry path: {}",
            path.display()
        ));
    }

    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "unsafe zip entry path escapes working_dir: {}",
            path.display()
        ));
    }

    Ok(())
}

fn reject_symlink_entry(entry: &zip::read::ZipFile<'_>) -> Result<(), String> {
    const UNIX_FILE_TYPE_MASK: u32 = 0o170000;
    const UNIX_SYMLINK: u32 = 0o120000;

    if entry
        .unix_mode()
        .is_some_and(|mode| mode & UNIX_FILE_TYPE_MASK == UNIX_SYMLINK)
    {
        return Err(format!("unsafe symlink zip entry: {}", entry.name()));
    }

    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("failed to open {} for hashing: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|error| format!("failed to read {} for hashing: {error}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn copy_output<R>(
    reader: R,
    log: Arc<Mutex<File>>,
    run_id: String,
) -> thread::JoinHandle<Result<(), String>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            let bytes_read = reader
                .read_until(b'\n', &mut buffer)
                .map_err(|error| format!("failed to read docker output: {error}"))?;
            if bytes_read == 0 {
                return Ok(());
            }

            {
                let mut log = log
                    .lock()
                    .map_err(|_| "harness log lock poisoned".to_owned())?;
                log.write_all(&buffer)
                    .map_err(|error| format!("failed to write harness log: {error}"))?;
            }

            write_prefixed_console_line(&run_id, &buffer)?;
        }
    })
}

fn write_prefixed_console_line(run_id: &str, line: &[u8]) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(format!("[{run_id}] ").as_bytes())
        .and_then(|_| stdout.write_all(line))
        .map_err(|error| format!("failed to write harness console output: {error}"))?;

    if !line.ends_with(b"\n") {
        stdout
            .write_all(b"\n")
            .map_err(|error| format!("failed to write harness console newline: {error}"))?;
    }

    stdout
        .flush()
        .map_err(|error| format!("failed to flush harness console output: {error}"))
}

fn join_log_thread(handle: thread::JoinHandle<Result<(), String>>) -> Result<(), String> {
    handle
        .join()
        .map_err(|_| "harness log thread panicked".to_owned())?
}

#[derive(Debug, Serialize)]
struct RunResult {
    run_id: String,
    batch_id: String,
    status: RunStatus,
    harness_exit_code: Option<i32>,
    started_at: String,
    finished_at: String,
    duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    inputs: Option<RunInputs>,
    selection: RunSelection,
    resolved: RunResolved,
    artifacts: RunArtifacts,
}

#[derive(Debug, Serialize)]
struct RunInputs {
    test: String,
    initial_state_sha256: String,
    prompt_sha256: String,
}

#[derive(Debug, Serialize)]
struct RunSelection {
    harness: String,
    model: String,
}

#[derive(Debug, Serialize)]
struct RunResolved {
    harness: ResolvedHarness,
    model: ResolvedModel,
}

#[derive(Debug, Serialize)]
struct ResolvedHarness {
    image: String,
}

#[derive(Debug, Serialize)]
struct ResolvedModel {
    model_name: String,
    base_url: String,
}

#[derive(Debug, Serialize)]
struct RunArtifacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    working_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    harness_log: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunStatus {
    Completed,
    Failed,
}

fn write_results(run_dir: &Path, result: &RunResult) -> Result<(), String> {
    let path = run_dir.join("results.json");
    let file = File::create(&path)
        .map_err(|error| format!("failed to create results file {}: {error}", path.display()))?;
    serde_json::to_writer_pretty(file, result)
        .map_err(|error| format!("failed to write results file {}: {error}", path.display()))
}

fn batch_id(started_at: OffsetDateTime) -> Result<String, String> {
    started_at
        .format(RUN_ID_TIME_FORMAT)
        .map_err(|error| format!("failed to format batch timestamp: {error}"))
}

fn run_id(batch_id: &str, harness: &str, model: &str, test: &str) -> String {
    let harness = sanitize_fragment(harness);
    let model = sanitize_fragment(model);
    let test = sanitize_fragment(test);
    let suffix = short_suffix();
    format!("{batch_id}_{harness}_{model}_{test}_{suffix}")
}

fn sanitize_fragment(value: &str) -> String {
    let mut sanitized = String::new();
    let mut last_was_dash = false;

    for character in value.chars() {
        let next = if character.is_ascii_alphanumeric() {
            last_was_dash = false;
            Some(character.to_ascii_lowercase())
        } else if !last_was_dash {
            last_was_dash = true;
            Some('-')
        } else {
            None
        };

        if let Some(character) = next {
            sanitized.push(character);
        }
    }

    sanitized.trim_matches('-').to_owned()
}

fn short_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{:06x}", nanos & 0x00ff_ffff)
}

fn format_timestamp(timestamp: OffsetDateTime) -> Result<String, String> {
    timestamp
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| format!("failed to format timestamp: {error}"))
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

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
