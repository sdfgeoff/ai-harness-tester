import { formatDuration, formatTimestamp } from "../format";
import { runHref } from "../routes";
import type { BatchSummary } from "../types";

interface BatchDetailProps {
  summary: BatchSummary;
}

export function BatchDetail({ summary }: BatchDetailProps) {
  return (
    <section className="panel panel-stack">
      <div className="panel-header">
        <div>
          <p className="eyebrow">Batch</p>
          <h2>{summary.batch_id}</h2>
        </div>
      </div>

      <dl className="stats-grid">
        <Stat label="Started" value={formatTimestamp(summary.started_at)} />
        <Stat label="Finished" value={formatTimestamp(summary.finished_at)} />
        <Stat label="Duration" value={formatDuration(summary.duration_ms)} />
        <Stat label="Runs" value={String(summary.runs.length)} />
      </dl>

      <div className="definition-row">
        <span className="definition-label">Config</span>
        <code>{summary.config_path}</code>
      </div>

      <section className="subpanel">
        <div className="panel-header">
          <h3>Runs</h3>
        </div>
        <ul className="run-list">
          {summary.runs.map((run) => (
            <li key={run.run_id}>
              <a className="list-link" href={runHref(summary.batch_id, run.run_id)}>
                <span className="list-title">{run.run_id}</span>
                <span className="list-subtitle">{run.results_path}</span>
              </a>
            </li>
          ))}
        </ul>
      </section>
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
