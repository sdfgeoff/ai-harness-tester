use std::{collections::BTreeMap, fs, path::Path};

use orchestrator_core::{
    config::Config,
    models::{
        write_evaluation, EvaluationResult, EvaluationScore, EvaluationStatus, ResolvedEvaluator,
        RunError,
    },
    test_selection::{evaluator_image_tag, TestSelection},
    util::{duration_ms, format_timestamp},
};
use serde_json::Value;
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
            result: None,
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
    let output_file = output_dir.join("evaluation.json");
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
            docker_runner::ContainerStatus::Completed => match read_scored_evaluation(&output_file)
            {
                Ok(score) => EvaluationResult {
                    status: EvaluationStatus::Scored,
                    reason: None,
                    started_at,
                    finished_at,
                    duration_ms,
                    evaluator,
                    result: Some(score),
                    error: None,
                },
                Err(error) => EvaluationResult {
                    status: EvaluationStatus::Failed,
                    reason: None,
                    started_at,
                    finished_at,
                    duration_ms,
                    evaluator,
                    result: None,
                    error: Some(error),
                },
            },
            docker_runner::ContainerStatus::Failed => EvaluationResult {
                status: EvaluationStatus::Failed,
                reason: None,
                started_at,
                finished_at,
                duration_ms,
                evaluator,
                result: None,
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
                result: None,
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
            result: None,
            error: Some(RunError {
                kind: "container_failed".to_owned(),
                message: error,
            }),
        },
    };

    write_evaluation(run_dir, &evaluation)
}

fn read_scored_evaluation(path: &Path) -> Result<EvaluationScore, RunError> {
    let contents = fs::read_to_string(path).map_err(|error| {
        let kind = if error.kind() == std::io::ErrorKind::NotFound {
            "missing_output"
        } else {
            "invalid_json"
        };
        RunError {
            kind: kind.to_owned(),
            message: format!(
                "failed to read evaluator output {}: {error}",
                path.display()
            ),
        }
    })?;

    let value: Value = serde_json::from_str(&contents).map_err(|error| RunError {
        kind: "invalid_json".to_owned(),
        message: format!(
            "failed to parse evaluator output {}: {error}",
            path.display()
        ),
    })?;

    validate_evaluation_value(&value)
}

fn validate_evaluation_value(value: &Value) -> Result<EvaluationScore, RunError> {
    let mut errors = Vec::new();
    let Some(object) = value.as_object() else {
        return Err(RunError {
            kind: "invalid_schema".to_owned(),
            message: "evaluation output must be a JSON object".to_owned(),
        });
    };

    let score = match object.get("score") {
        Some(score) => validate_score_value(score, "score", &mut errors),
        None => {
            errors.push("missing required field 'score'".to_owned());
            None
        }
    };

    let breakdown = match object.get("breakdown") {
        Some(breakdown) => validate_breakdown_value(breakdown, &mut errors),
        None => None,
    };

    if errors.is_empty() {
        Ok(EvaluationScore {
            score: score.expect("score should be present when validation passes"),
            breakdown,
        })
    } else {
        Err(RunError {
            kind: "invalid_schema".to_owned(),
            message: errors.join("; "),
        })
    }
}

fn validate_breakdown_value(
    value: &Value,
    errors: &mut Vec<String>,
) -> Option<BTreeMap<String, f64>> {
    let Some(object) = value.as_object() else {
        errors.push("'breakdown' must be an object".to_owned());
        return None;
    };

    let mut breakdown = BTreeMap::new();
    for (criterion, value) in object {
        if let Some(score) = validate_score_value(value, &format!("breakdown.{criterion}"), errors)
        {
            breakdown.insert(criterion.clone(), score);
        }
    }

    Some(breakdown)
}

fn validate_score_value(value: &Value, field: &str, errors: &mut Vec<String>) -> Option<f64> {
    let Some(score) = value.as_f64() else {
        errors.push(format!("'{field}' must be a number"));
        return None;
    };

    if !score.is_finite() {
        errors.push(format!("'{field}' must be finite"));
        return None;
    }

    if !(0.0..=1.0).contains(&score) {
        errors.push(format!("'{field}' must be between 0 and 1"));
        return None;
    }

    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_score_and_breakdown() {
        let score = validate_evaluation_value(&serde_json::json!({
            "score": 0.9,
            "breakdown": {
                "correctness": 1.0,
                "performance": 0.8
            }
        }))
        .expect("valid evaluation");

        assert_eq!(score.score, 0.9);
        assert_eq!(
            score
                .breakdown
                .as_ref()
                .and_then(|b| b.get("correctness"))
                .copied(),
            Some(1.0)
        );
    }

    #[test]
    fn allows_empty_breakdown() {
        let score = validate_evaluation_value(&serde_json::json!({
            "score": 0.9,
            "breakdown": {}
        }))
        .expect("valid evaluation");

        assert_eq!(score.breakdown, Some(BTreeMap::new()));
    }

    #[test]
    fn reports_all_schema_errors() {
        let error = validate_evaluation_value(&serde_json::json!({
            "score": 2.0,
            "breakdown": {
                "correctness": -1.0,
                "performance": "fast"
            }
        }))
        .expect_err("invalid evaluation");

        assert_eq!(error.kind, "invalid_schema");
        assert!(error.message.contains("'score' must be between 0 and 1"));
        assert!(error
            .message
            .contains("'breakdown.correctness' must be between 0 and 1"));
        assert!(error
            .message
            .contains("'breakdown.performance' must be a number"));
    }
}
