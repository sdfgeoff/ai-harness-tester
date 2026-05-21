use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// Inspect a Docker image and return its ID.
pub fn inspect_image(name: &str, image: &str) -> Result<String, String> {
    let output = Command::new("docker")
        .arg("image")
        .arg("inspect")
        .arg("--format")
        .arg("{{.Id}}")
        .arg(image)
        .output()
        .map_err(|error| format!("failed to inspect Docker image for '{name}': {error}"))?;

    if output.status.success() {
        let image_id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        tracing::info!(name, image, image_id = %image_id, "Docker image inspected");
        Ok(image_id)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Docker image for '{name}' does not exist locally: {image}: {}",
            stderr.trim()
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkMode {
    Host,
    None,
}

#[derive(Debug, Clone)]
pub struct Mount {
    pub source: PathBuf,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub container_name: String,
    pub image: String,
    pub network: NetworkMode,
    pub mounts: Vec<Mount>,
    pub workdir: Option<String>,
    pub env: Vec<(String, String)>,
    pub log_path: PathBuf,
    pub console_prefix: Option<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerStatus {
    Completed,
    Failed,
    TimedOut,
}

#[derive(Debug)]
pub struct RunResult {
    pub status: ContainerStatus,
    pub exit_code: Option<i32>,
}

pub fn run(config: &RunConfig) -> Result<RunResult, String> {
    let log = Arc::new(Mutex::new(File::create(&config.log_path).map_err(
        |error| {
            format!(
                "failed to create container log {}: {error}",
                config.log_path.display()
            )
        },
    )?));

    let mut command = Command::new("docker");
    command.args(build_docker_args(config)?);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command
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

    let stdout_thread = copy_output(stdout, Arc::clone(&log), config.console_prefix.clone());
    let stderr_thread = copy_output(stderr, log, config.console_prefix.clone());

    let start = Instant::now();
    let (status, exit_code) = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let exit_code = status.code();
                let container_status = if exit_code == Some(0) {
                    ContainerStatus::Completed
                } else {
                    ContainerStatus::Failed
                };
                break (container_status, exit_code);
            }
            Ok(None) => {
                if start.elapsed() >= config.timeout {
                    tracing::warn!(
                        container_name = %config.container_name,
                        timeout_seconds = config.timeout.as_secs(),
                        "container exceeded timeout, killing"
                    );
                    let _ = Command::new("docker")
                        .arg("kill")
                        .arg(&config.container_name)
                        .output();
                    let _ = child.kill();
                    let _ = child.wait();
                    break (ContainerStatus::TimedOut, None);
                }
                thread::sleep(Duration::from_millis(500));
            }
            Err(error) => {
                return Err(format!("failed to check docker status: {error}"));
            }
        }
    };

    join_log_thread(stdout_thread)?;
    join_log_thread(stderr_thread)?;

    Ok(RunResult { status, exit_code })
}

fn build_docker_args(config: &RunConfig) -> Result<Vec<String>, String> {
    let mut args = vec![
        "run".to_owned(),
        "--rm".to_owned(),
        "--name".to_owned(),
        config.container_name.clone(),
        "--network".to_owned(),
        match config.network {
            NetworkMode::Host => "host".to_owned(),
            NetworkMode::None => "none".to_owned(),
        },
    ];

    for mount in &config.mounts {
        args.push("--volume".to_owned());
        args.push(format_mount_arg(mount)?);
    }

    if let Some(workdir) = &config.workdir {
        args.push("--workdir".to_owned());
        args.push(workdir.clone());
    }

    for (key, value) in &config.env {
        args.push("--env".to_owned());
        args.push(format!("{key}={value}"));
    }

    args.push(config.image.clone());
    Ok(args)
}

fn format_mount_arg(mount: &Mount) -> Result<String, String> {
    let source = canonicalize_mount_source(&mount.source)?;
    let mut spec = format!("{}:{}", source.display(), mount.target);
    if mount.read_only {
        spec.push_str(":ro");
    }
    Ok(spec)
}

fn canonicalize_mount_source(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|error| {
        format!(
            "failed to canonicalize mount source {}: {error}",
            path.display()
        )
    })
}

fn copy_output<R>(
    reader: R,
    log: Arc<Mutex<File>>,
    console_prefix: Option<String>,
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
                    .map_err(|_| "container log lock poisoned".to_owned())?;
                log.write_all(&buffer)
                    .map_err(|error| format!("failed to write container log: {error}"))?;
            }

            if let Some(prefix) = console_prefix.as_deref() {
                write_prefixed_console_line(prefix, &buffer)?;
            }
        }
    })
}

fn write_prefixed_console_line(prefix: &str, line: &[u8]) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(format!("[{prefix}] ").as_bytes())
        .and_then(|_| stdout.write_all(line))
        .map_err(|error| format!("failed to write container console output: {error}"))?;

    if !line.ends_with(b"\n") {
        stdout
            .write_all(b"\n")
            .map_err(|error| format!("failed to write container console newline: {error}"))?;
    }

    stdout
        .flush()
        .map_err(|error| format!("failed to flush container console output: {error}"))
}

fn join_log_thread(handle: thread::JoinHandle<Result<(), String>>) -> Result<(), String> {
    handle
        .join()
        .map_err(|_| "container log thread panicked".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_host_network_args() {
        let config = RunConfig {
            container_name: "test-container".to_owned(),
            image: "test-image:latest".to_owned(),
            network: NetworkMode::Host,
            mounts: Vec::new(),
            workdir: Some("/workdir".to_owned()),
            env: vec![("WORKDIR".to_owned(), "/workdir".to_owned())],
            log_path: PathBuf::from("container.log"),
            console_prefix: Some("run-1".to_owned()),
            timeout: Duration::from_secs(30),
        };

        let args = build_docker_args(&config).unwrap();
        assert_eq!(
            args,
            vec![
                "run",
                "--rm",
                "--name",
                "test-container",
                "--network",
                "host",
                "--workdir",
                "/workdir",
                "--env",
                "WORKDIR=/workdir",
                "test-image:latest",
            ]
        );
    }

    #[test]
    fn builds_read_only_mount_args() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("input.txt");
        std::fs::write(&source, "hello").unwrap();

        let mount = Mount {
            source,
            target: "/input/data.txt".to_owned(),
            read_only: true,
        };

        let arg = format_mount_arg(&mount).unwrap();
        assert!(arg.ends_with(":/input/data.txt:ro"));
    }
}
