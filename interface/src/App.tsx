import { useEffect, useState } from "react";
import { fetchResultsIndex } from "./data";
import type { ResultsIndex } from "./types";

export function App() {
  const [index, setIndex] = useState<ResultsIndex | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    fetchResultsIndex()
      .then(setIndex)
      .catch((loadError: unknown) => {
        setError(loadError instanceof Error ? loadError.message : "Failed to load results index");
      });
  }, []);

  return (
    <main className="app-shell">
      <header className="app-header">
        <div>
          <p className="eyebrow">Harness Test</p>
          <h1>Results Browser</h1>
        </div>
      </header>

      <section className="panel">
        <h2>Batches</h2>
        {error ? <p className="error-text">{error}</p> : null}
        {!error && index === null ? <p className="muted-text">Loading…</p> : null}
        {index !== null ? (
          <ul className="batch-list">
            {index.batches.map((batch) => (
              <li key={batch.batch_id}>
                <code>{batch.batch_id}</code>
                <span>{batch.summary_path}</span>
              </li>
            ))}
          </ul>
        ) : null}
      </section>
    </main>
  );
}
