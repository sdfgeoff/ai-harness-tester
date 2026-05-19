use clap::Parser;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
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
        /// Docker image tag or ID to run.
        image: String,
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
        CommandName::RunImage { image, test } => run_image(&image, test.as_deref()),
    }
}

fn run_image(image: &str, test: Option<&str>) -> Result<(), String> {
    let selected_test = test.map(load_test_selection).transpose()?;
    let started_at = OffsetDateTime::now_utc();
    let started = Instant::now();
    let run_id = run_id(started_at, image)?;
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

    println!("running Docker image: {image}");

    let mut child = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg(image)
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
        artifacts: RunArtifacts {
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

#[derive(Debug)]
struct TestSelection {
    name: String,
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
        initial_state_sha256: sha256_file(&initial_state)?,
        prompt_sha256: sha256_file(&prompt)?,
    })
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
    artifacts: RunArtifacts,
}

#[derive(Debug, Serialize)]
struct RunInputs {
    test: String,
    initial_state_sha256: String,
    prompt_sha256: String,
}

#[derive(Debug, Serialize)]
struct RunArtifacts {
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
}
