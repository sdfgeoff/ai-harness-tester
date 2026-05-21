import { useEffect, useMemo, useState } from "react";
import {
  fetchBatchSummary,
  fetchResultsIndex,
  fetchRunEvaluation,
  fetchRunResults,
} from "./data";
import { useHashRoute } from "./useHashRoute";
import { BatchList } from "./views/BatchList";
import { BatchDetail } from "./views/BatchDetail";
import { RunDetail } from "./views/RunDetail";
import type { BatchSummary, ResultsIndex, RunEvaluation, RunResults } from "./types";

export function App() {
  const route = useHashRoute();
  const [index, setIndex] = useState<ResultsIndex | null>(null);
  const [indexError, setIndexError] = useState<string | null>(null);
  const [summary, setSummary] = useState<BatchSummary | null>(null);
  const [summaryError, setSummaryError] = useState<string | null>(null);
  const [results, setResults] = useState<RunResults | null>(null);
  const [evaluation, setEvaluation] = useState<RunEvaluation | null>(null);
  const [runError, setRunError] = useState<string | null>(null);

  useEffect(() => {
    fetchResultsIndex()
      .then(setIndex)
      .catch((loadError: unknown) => {
        setIndexError(
          loadError instanceof Error ? loadError.message : "Failed to load results index",
        );
      });
  }, []);

  const selectedBatch = useMemo(() => {
    if (!index) {
      return null;
    }
    if (route.kind === "home") {
      return index.batches[0] ?? null;
    }
    return index.batches.find((batch) => batch.batch_id === route.batchId) ?? null;
  }, [index, route]);

  useEffect(() => {
    if (!selectedBatch) {
      setSummary(null);
      setSummaryError(null);
      return;
    }

    setSummary(null);
    setSummaryError(null);
    fetchBatchSummary(selectedBatch.summary_path)
      .then(setSummary)
      .catch((loadError: unknown) => {
        setSummaryError(
          loadError instanceof Error ? loadError.message : "Failed to load batch summary",
        );
      });
  }, [selectedBatch]);

  const selectedRunReference = useMemo(() => {
    if (route.kind !== "run" || !summary) {
      return null;
    }
    return summary.runs.find((run) => run.run_id === route.runId) ?? null;
  }, [route, summary]);

  useEffect(() => {
    if (route.kind !== "run" || !selectedRunReference) {
      setResults(null);
      setEvaluation(null);
      setRunError(null);
      return;
    }

    setResults(null);
    setEvaluation(null);
    setRunError(null);

    Promise.all([
      fetchRunResults(route.batchId, selectedRunReference.results_path),
      fetchRunEvaluation(route.batchId, selectedRunReference.evaluation_path),
    ])
      .then(([loadedResults, loadedEvaluation]) => {
        setResults(loadedResults);
        setEvaluation(loadedEvaluation);
      })
      .catch((loadError: unknown) => {
        setRunError(loadError instanceof Error ? loadError.message : "Failed to load run");
      });
  }, [route, selectedRunReference]);

  return (
    <main className="app-shell">
      <header className="app-header">
        <div>
          <p className="eyebrow">Harness Test</p>
          <h1>Results Browser</h1>
        </div>
      </header>

      <div className="layout-grid">
        <div className="sidebar-column">
          {indexError ? <ErrorPanel title="Index error" message={indexError} /> : null}
          {!indexError && index === null ? <LoadingPanel title="Batches" /> : null}
          {index ? <BatchList index={index} selectedBatchId={selectedBatch?.batch_id} /> : null}
        </div>

        <div className="detail-column">
          {summaryError ? <ErrorPanel title="Batch error" message={summaryError} /> : null}
          {!summaryError && selectedBatch && summary === null ? <LoadingPanel title="Batch" /> : null}
          {summary ? <BatchDetail summary={summary} /> : null}
          {runError ? <ErrorPanel title="Run error" message={runError} /> : null}
          {route.kind === "run" && !runError && (!results || !evaluation) ? (
            <LoadingPanel title="Run" />
          ) : null}
          {route.kind === "run" && selectedRunReference && results && evaluation ? (
            <RunDetail
              batchId={route.batchId}
              runReference={selectedRunReference}
              results={results}
              evaluation={evaluation}
            />
          ) : null}
        </div>
      </div>
    </main>
  );
}

function LoadingPanel({ title }: { title: string }) {
  return (
    <section className="panel">
      <div className="panel-header">
        <h2>{title}</h2>
      </div>
      <p className="muted-text">Loading…</p>
    </section>
  );
}

function ErrorPanel({ title, message }: { title: string; message: string }) {
  return (
    <section className="panel">
      <div className="panel-header">
        <h2>{title}</h2>
      </div>
      <p className="error-text">{message}</p>
    </section>
  );
}
