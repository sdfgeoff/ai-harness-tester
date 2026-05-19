use std::{

    process::Stdio,
    time::Duration,
};
use tokio::process::Child;

/// Inspect a Docker image and return its ID.
pub fn inspect_image(harness_name: &str, image: &str) -> Result<String, String> {
    let output = std::process::Command::new("docker")
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
        tracing::info!(harness = harness_name, image = image, image_id = %image_id, "Docker image inspected");
        Ok(image_id)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Docker image for harness '{harness_name}' does not exist locally: {image}: {}",
            stderr.trim()
        ))
    }
}

/// Configuration for running a Docker container.
#[derive(Debug)]
pub struct RunConfig {
    pub container_name: String,
    pub image: String,
    pub workdir: Option<std::path::PathBuf>,
    pub prompt_path: Option<std::path::PathBuf>,
    pub llm_url: String,
    pub llm_api_key: String,
}

/// Result of running a Docker container.
#[derive(Debug)]
pub struct RunResult {
    pub status: ContainerStatus,
    pub exit_code: Option<i32>,
}

/// The status of a container run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerStatus {
    Completed,
    Failed,
    TimedOut,
}

/// Spawn a Docker container and return the child process.
pub fn spawn(config: &RunConfig) -> Result<Child, String> {
    let mut command = tokio::process::Command::new("docker");
    command
        .arg("run")
        .arg("--rm")
        .arg("--name")
        .arg(&config.container_name);

    if let Some(ref workdir) = config.workdir {
        let mount_source = std::fs::canonicalize(workdir).map_err(|error| {
            format!("failed to canonicalize working directory {}: {error}", workdir.display())
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
        .arg(format!("LLM_URL={}", config.llm_url))
        .arg("--env")
        .arg(format!("LLM_API_KEY={}", config.llm_api_key));

    if let Some(ref prompt_path) = config.prompt_path {
        let mount_source = std::fs::canonicalize(prompt_path).map_err(|error| {
            format!("failed to canonicalize prompt {}: {error}", prompt_path.display())
        })?;
        command
            .arg("--volume")
            .arg(format!("{}:/prompt/PROMPT.md:ro", mount_source.display()))
            .arg("--env")
            .arg("INITIAL_PROMPT_FILE=/prompt/PROMPT.md");
    }

    command
        .arg(&config.image)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    command.spawn().map_err(|error| format!("failed to start docker: {error}"))
}

/// Wait for a child process with a timeout.
pub async fn wait_with_timeout(mut child: Child, timeout: Duration, run_id: &str) -> RunResult {
    let timeout_instant = std::time::Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let exit_code = status.code();
                let container_status = if exit_code == Some(0) {
                    ContainerStatus::Completed
                } else {
                    ContainerStatus::Failed
                };
                tracing::info!(run_id = %run_id, exit_code = ?exit_code, status = ?container_status, "container exited");
                return RunResult { status: container_status, exit_code };
            }
            Ok(None) => {
                if std::time::Instant::now() >= timeout_instant {
                    tracing::warn!(run_id = %run_id, timeout_seconds = ?timeout.as_secs(), "container exceeded timeout, killing");
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return RunResult { status: ContainerStatus::TimedOut, exit_code: None };
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(error) => {
                tracing::error!(run_id = %run_id, error = %error, "error waiting for container");
                return RunResult { status: ContainerStatus::Failed, exit_code: None };
            }
        }
    }
}

/// Kill a container.
pub async fn kill(mut child: Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}
