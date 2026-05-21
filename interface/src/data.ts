import { parseProxyNdjson, type ParsedProxyLog } from "./proxy";
import type { BatchSummary, ResultsIndex, RunEvaluation, RunResults } from "./types";

async function fetchJson<T>(path: string): Promise<T> {
  const response = await fetch(path);
  if (!response.ok) {
    throw new Error(`Failed to load ${path}: ${response.status} ${response.statusText}`);
  }
  return (await response.json()) as T;
}

async function fetchText(path: string): Promise<string> {
  const response = await fetch(path);
  if (!response.ok) {
    throw new Error(`Failed to load ${path}: ${response.status} ${response.statusText}`);
  }
  return await response.text();
}

export function fetchResultsIndex(): Promise<ResultsIndex> {
  return fetchJson<ResultsIndex>("/results/index.json");
}

export function fetchBatchSummary(path: string): Promise<BatchSummary> {
  return fetchJson<BatchSummary>(normalizeArtifactPath(path));
}

export function fetchRunResults(batchId: string, path: string): Promise<RunResults> {
  return fetchJson<RunResults>(resolveBatchArtifactPath(batchId, path));
}

export function fetchRunEvaluation(batchId: string, path: string): Promise<RunEvaluation> {
  return fetchJson<RunEvaluation>(resolveBatchArtifactPath(batchId, path));
}

export async function fetchProxyLog(
  batchId: string,
  resultsPath: string,
  proxyLogPath: string,
): Promise<ParsedProxyLog> {
  const path = resolveRunArtifactPath(batchId, resultsPath, proxyLogPath);
  const contents = await fetchText(path);
  return parseProxyNdjson(contents);
}

export function resolveBatchArtifactPath(batchId: string, relativePath: string): string {
  return normalizeArtifactPath(`/results/${batchId}/${relativePath}`);
}

export function resolveRunArtifactPath(
  batchId: string,
  resultsPath: string,
  runRelativePath: string,
): string {
  const runDirectory = resultsPath.replace(/\/results\.json$/, "");
  return resolveBatchArtifactPath(batchId, `${runDirectory}/${runRelativePath}`);
}

export function normalizeArtifactPath(path: string): string {
  return path.startsWith("/") ? path : `/${path}`;
}
