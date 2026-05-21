use std::{fs, path::Path};

use orchestrator_core::{
    config::Config,
    models::{write_evaluation, EvaluationResult, EvaluationStatus, ResolvedEvaluator, RunError},
    test_selection::{evaluator_image_tag, TestSelection},
    util::{duration_ms, format_timestamp},
};
use time::OffsetDateTime;

pub fn write_skipped_evaluation(run_dir: &Path) -> Result<(), String> {
    write_evaluation(
        run_dir,
        &EvaluationResult {
            status: EvaluationStatus::Skipped,
            reason: Some("run_not_completed".to_owned()),
            started_at: None,
            finished_at: None,
            duration_ms: None,
            evaluator: None,
            error: None,
        },
    )
}

pub fn evaluate_completed_run(
    config: &Config,
    run_id: &str,
    run_dir: &Path,
    test: &TestSelection,
    evaluator_image_id: &str,
) -> Result<(), String> {
    let started_at = OffsetDateTime::now_utc();
    let started = std::time::Instant::now();
    let logs_dir = run_dir.join("logs");
    let log_path = logs_dir.join("evaluator.log");
    let output_dir = run_dir.join("evaluation_output");
    fs::create_dir_all(&output_dir).map_err(|error| {
        format!(
            "failed to create evaluation output directory {}: {error}",
            output_dir.display()
        )
    })?;

    let image = evaluator_image_tag(&test.name);
    let run_result = docker_runner::run(&docker_runner::RunConfig {
        container_name: format!("harness-test-eval-{run_id}"),
        image: image.clone(),
        network: docker_runner::NetworkMode::Host,
        mounts: vec![
            docker_runner::Mount {
                source: run_dir.to_path_buf(),
                target: "/run".to_owned(),
                read_only: true,
            },
            docker_runner::Mount {
                source: test.test_dir.clone(),
                target: "/evaluator".to_owned(),
                read_only: true,
            },
            docker_runner::Mount {
                source: output_dir,
                target: "/output".to_owned(),
                read_only: false,
            },
        ],
        workdir: None,
        env: Vec::new(),
        log_path,
        console_prefix: Some(format!("{run_id}:evaluator")),
        timeout: std::time::Duration::from_secs(config.evaluation_timeout_seconds),
    });

    let finished_at = OffsetDateTime::now_utc();
    let evaluator = Some(ResolvedEvaluator {
        image,
        image_id: Some(evaluator_image_id.to_owned()),
    });
    let started_at = Some(format_timestamp(started_at)?);
    let finished_at = Some(format_timestamp(finished_at)?);
    let duration_ms = Some(duration_ms(started.elapsed()));

    let evaluation = match run_result {
        Ok(result) => match result.status {
            docker_runner::ContainerStatus::Completed => EvaluationResult {
                status: EvaluationStatus::Completed,
                reason: None,
                started_at,
                finished_at,
                duration_ms,
                evaluator,
                error: None,
            },
            docker_runner::ContainerStatus::Failed => EvaluationResult {
                status: EvaluationStatus::Failed,
                reason: None,
                started_at,
                finished_at,
                duration_ms,
                evaluator,
                error: Some(RunError {
                    kind: "container_failed".to_owned(),
                    message: format!("Evaluator exited with code {:?}", result.exit_code),
                }),
            },
            docker_runner::ContainerStatus::TimedOut => EvaluationResult {
                status: EvaluationStatus::Failed,
                reason: None,
                started_at,
                finished_at,
                duration_ms,
                evaluator,
                error: Some(RunError {
                    kind: "timed_out".to_owned(),
                    message: format!(
                        "Evaluation exceeded timeout of {} seconds",
                        config.evaluation_timeout_seconds
                    ),
                }),
            },
        },
        Err(error) => EvaluationResult {
            status: EvaluationStatus::Failed,
            reason: None,
            started_at,
            finished_at,
            duration_ms,
            evaluator,
            error: Some(RunError {
                kind: "container_failed".to_owned(),
                message: error,
            }),
        },
    };

    write_evaluation(run_dir, &evaluation)
}
