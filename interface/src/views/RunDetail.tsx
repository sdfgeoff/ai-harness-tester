import { ProxyLogViewer } from "./ProxyLogViewer";
import { formatDuration, formatScore, formatTimestamp } from "../format";
import { resolveBatchArtifactPath, resolveRunArtifactPath } from "../data";
import type { RunEvaluation, RunReference, RunResults } from "../types";

interface RunDetailProps {
  batchId: string;
  runReference: RunReference;
  results: RunResults;
  evaluation: RunEvaluation;
}

export function RunDetail({ batchId, runReference, results, evaluation }: RunDetailProps) {
  const resultsPath = resolveBatchArtifactPath(batchId, runReference.results_path);
  const evaluationPath = resolveBatchArtifactPath(batchId, runReference.evaluation_path);

  return (
    <section className="panel panel-stack">
      <div className="panel-header">
        <div>
          <p className="eyebrow">Run</p>
          <h2>{results.run_id}</h2>
        </div>
      </div>

      <dl className="stats-grid">
        <Stat label="Run status" value={results.status} />
        <Stat label="Eval status" value={evaluation.status} />
        <Stat label="Score" value={formatScore(evaluation.result?.score)} />
        <Stat label="Duration" value={formatDuration(results.duration_ms)} />
      </dl>

      <section className="subpanel">
        <h3>Selection</h3>
        <dl className="kv-grid">
          <KV label="Test" value={results.selection.test} />
          <KV label="Harness" value={results.selection.harness} />
          <KV label="Model" value={results.selection.model} />
        </dl>
      </section>

      <section className="subpanel">
        <h3>Timing</h3>
        <dl className="kv-grid">
          <KV label="Started" value={formatTimestamp(results.started_at)} />
          <KV label="Finished" value={formatTimestamp(results.finished_at)} />
          <KV label="Evaluation started" value={formatTimestamp(evaluation.started_at)} />
          <KV label="Evaluation finished" value={formatTimestamp(evaluation.finished_at)} />
        </dl>
      </section>

      <section className="subpanel">
        <h3>Metrics</h3>
        <dl className="kv-grid">
          {Object.entries(results.metrics).map(([key, value]) => (
            <KV key={key} label={key} value={value === null ? "null" : String(value)} />
          ))}
        </dl>
      </section>

      <section className="subpanel">
        <h3>Evaluation</h3>
        {evaluation.result?.breakdown ? (
          <dl className="kv-grid">
            {Object.entries(evaluation.result.breakdown).map(([key, value]) => (
              <KV key={key} label={key} value={formatScore(value)} />
            ))}
          </dl>
        ) : (
          <p className="muted-text">No breakdown recorded.</p>
        )}
        {evaluation.error ? (
          <div className="error-block">
            <strong>{evaluation.error.kind}</strong>
            <p>{evaluation.error.message}</p>
          </div>
        ) : null}
      </section>

      <section className="subpanel">
        <h3>Artifacts</h3>
        <ul className="artifact-list">
          <ArtifactLink label="results.json" path={resultsPath} />
          <ArtifactLink label="evaluation.json" path={evaluationPath} />
          <ArtifactLink
            label="harness.log"
            path={resolveRunArtifactPath(batchId, runReference.results_path, results.artifacts.harness_log)}
          />
          <ArtifactLink
            label="proxy.ndjson"
            path={resolveRunArtifactPath(batchId, runReference.results_path, results.artifacts.proxy_log)}
          />
          {results.artifacts.prompt ? (
          <ArtifactLink
            label="PROMPT.md"
            path={resolveRunArtifactPath(batchId, runReference.results_path, results.artifacts.prompt)}
          />
          ) : null}
          {results.artifacts.working_dir ? (
            <ArtifactLink
              label="working_dir"
              path={resolveRunArtifactPath(
                batchId,
                runReference.results_path,
                results.artifacts.working_dir,
              )}
            />
          ) : null}
          <ArtifactLink
            label="evaluation_output/evaluation.json"
            path={resolveRunArtifactPath(
              batchId,
              runReference.results_path,
              "evaluation_output/evaluation.json",
            )}
          />
        </ul>
      </section>

      <ProxyLogViewer
        batchId={batchId}
        resultsPath={runReference.results_path}
        proxyLogPath={results.artifacts.proxy_log}
      />
    </section>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="stat-card">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function KV({ label, value }: { label: string; value: string }) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </>
  );
}

function ArtifactLink({ label, path }: { label: string; path: string }) {
  return (
    <li>
      <a href={path} target="_blank" rel="noreferrer">
        {label}
      </a>
      <code>{path}</code>
    </li>
  );
}
