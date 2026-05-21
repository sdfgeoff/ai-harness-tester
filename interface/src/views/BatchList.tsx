import { batchHref } from "../routes";
import type { ResultsIndex } from "../types";

interface BatchListProps {
  index: ResultsIndex;
  selectedBatchId?: string;
}

export function BatchList({ index, selectedBatchId }: BatchListProps) {
  return (
    <section className="panel">
      <div className="panel-header">
        <h2>Batches</h2>
        <span className="panel-meta">{index.batches.length}</span>
      </div>
      <ul className="batch-list">
        {index.batches.map((batch) => (
          <li key={batch.batch_id}>
            <a
              className={batch.batch_id === selectedBatchId ? "list-link is-active" : "list-link"}
              href={batchHref(batch.batch_id)}
            >
              <span className="list-title">
                <code>{batch.batch_id}</code>
              </span>
              <span className="list-subtitle">{batch.summary_path}</span>
            </a>
          </li>
        ))}
      </ul>
    </section>
  );
}
