export interface ResultsIndexBatch {
  batch_id: string;
  summary_path: string;
}

export interface ResultsIndex {
  batches: ResultsIndexBatch[];
}

export interface BatchSummary {
  batch_id: string;
  started_at: string;
  finished_at: string;
  duration_ms: number;
  config_path: string;
  runs: RunReference[];
}

export interface RunReference {
  run_id: string;
  results_path: string;
  evaluation_path: string;
}

export interface RunResults {
  run_id: string;
  batch_id: string;
  status: string;
  harness_exit_code: number | null;
  error: RunError | null;
  started_at: string;
  finished_at: string;
  duration_ms: number;
  inputs?: RunInputs | null;
  selection: RunSelection;
  resolved: RunResolved;
  metrics: Record<string, number | null>;
  artifacts: RunArtifacts;
}

export interface RunError {
  kind: string;
  message: string;
}

export interface RunInputs {
  initial_state_sha256: string;
  prompt_sha256: string;
}

export interface RunSelection {
  test: string;
  harness: string;
  model: string;
}

export interface RunResolved {
  harness: {
    image: string;
    image_id?: string | null;
  };
  model: {
    model_name: string;
    base_url: string;
  };
}

export interface RunArtifacts {
  working_dir?: string | null;
  prompt?: string | null;
  harness_log: string;
  proxy_log: string;
}

export interface RunEvaluation {
  status: "skipped" | "scored" | "failed";
  reason?: string | null;
  started_at?: string | null;
  finished_at?: string | null;
  duration_ms?: number | null;
  evaluator?: {
    image: string;
    image_id?: string | null;
  } | null;
  result?: {
    score: number;
    breakdown?: Record<string, number> | null;
  } | null;
  error?: RunError | null;
}
