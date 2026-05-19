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
        /// Harness profile name from config.
        #[arg(long)]
        harness: String,
        /// Config file path.
        #[arg(long, default_value = "config.json")]
        config: PathBuf,
        /// Test folder name under tests/.
        #[arg(long)]
        test: Option<String>,
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
            harness,
            config,
            test,
        } => {
            let config = load_config(&config)?;
            let harness_profile = config
                .harnesses
                .get(&harness)
                .ok_or_else(|| format!("unknown harness profile '{harness}'"))?;
            run_image(&harness, harness_profile, test.as_deref())
        }
    }
}

fn run_image(
    harness_name: &str,
    harness: &HarnessProfile,
    test: Option<&str>,
) -> Result<(), String> {
    let selected_test = test.map(load_test_selection).transpose()?;
    let started_at = OffsetDateTime::now_utc();
    let started = Instant::now();
    let run_id = run_id(started_at, harness_name)?;
    let run_dir = PathBuf::from("results").join(&run_id);
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
    let mut child = command
        .arg(&harness.image)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start docker: {error}"))?;
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

    let duration = started.elapsed();
    let finished_at = OffsetDateTime::now_utc();
    let harness_exit_code = status.code();
    let run_status = match harness_exit_code {
        Some(0) => RunStatus::Completed,
        _ => RunStatus::Failed,
    };
    let result = RunResult {
        run_id: run_id.clone(),
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
        },
        resolved: RunResolved {
            harness: ResolvedHarness {
                image: harness.image.clone(),
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
            Ok(())
        }
        Some(code) => Err(format!(
            "container exited with status {code} after {:.2?}",
            duration
        )),
        None => Err(format!(
            "container terminated without an exit code after {:.2?}",
            duration
        )),
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    harnesses: BTreeMap<String, HarnessProfile>,
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
}

#[derive(Debug, Serialize)]
struct RunResolved {
    harness: ResolvedHarness,
}

#[derive(Debug, Serialize)]
struct ResolvedHarness {
    image: String,
}

#[derive(Debug, Serialize)]
struct RunArtifacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    working_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    harness_log: String,
}

#[derive(Debug, Serialize)]
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

fn run_id(started_at: OffsetDateTime, image: &str) -> Result<String, String> {
    let timestamp = started_at
        .format(RUN_ID_TIME_FORMAT)
        .map_err(|error| format!("failed to format run timestamp: {error}"))?;
    let image = sanitize_fragment(image);
    let suffix = short_suffix();
    Ok(format!("{timestamp}_{image}_{suffix}"))
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
}
