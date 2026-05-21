export interface HomeRoute {
  kind: "home";
}

export interface BatchRoute {
  kind: "batch";
  batchId: string;
}

export interface RunRoute {
  kind: "run";
  batchId: string;
  runId: string;
}

export type Route = HomeRoute | BatchRoute | RunRoute;

export function parseHashRoute(hash: string): Route {
  const normalized = hash.replace(/^#/, "");
  const segments = normalized.split("/").filter(Boolean).map(decodeURIComponent);

  if (segments.length === 0) {
    return { kind: "home" };
  }

  if (segments[0] !== "batches" || segments.length < 2) {
    return { kind: "home" };
  }

  const batchId = segments[1];
  if (segments.length === 2) {
    return { kind: "batch", batchId };
  }

  if (segments[2] === "runs" && segments[3]) {
    return { kind: "run", batchId, runId: segments[3] };
  }

  return { kind: "batch", batchId };
}

export function batchHref(batchId: string): string {
  return `#/batches/${encodeURIComponent(batchId)}`;
}

export function runHref(batchId: string, runId: string): string {
  return `#/batches/${encodeURIComponent(batchId)}/runs/${encodeURIComponent(runId)}`;
}
