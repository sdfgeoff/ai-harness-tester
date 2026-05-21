import type { RunError } from "./types";

type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

interface ProxyRecordBase {
  record_type: string;
  request_id: string;
}

export interface ProxyRequestStartRecord extends ProxyRecordBase {
  record_type: "request_start";
  started_at?: string;
  kind?: string;
  method?: string;
  path?: string;
  original_model?: string;
  upstream_model?: string;
  request_body?: JsonValue;
}

export interface ProxyUsage {
  input_tokens?: number | null;
  output_tokens?: number | null;
  total_tokens?: number | null;
  cache_read_tokens?: number | null;
  cache_write_tokens?: number | null;
}

export interface ProxyRequestEndRecord extends ProxyRecordBase {
  record_type: "request_end";
  finished_at?: string;
  duration_ms?: number;
  kind?: string;
  method?: string;
  path?: string;
  original_model?: string;
  upstream_model?: string;
  status_code?: number;
  response_body?: JsonValue;
  usage?: ProxyUsage | null;
  error?: string | null;
}

export interface ProxyStreamEventRecord extends ProxyRecordBase {
  record_type: "stream_event";
  received_at?: string;
  event?: string;
  data_raw?: string;
}

export type ProxyLogRecord =
  | ProxyRequestStartRecord
  | ProxyRequestEndRecord
  | ProxyStreamEventRecord
  | (ProxyRecordBase & Record<string, JsonValue | undefined>);

export interface ProxyParseIssue {
  lineNumber: number;
  message: string;
  rawLine: string;
}

export interface ProxyDerivedFragment {
  kind: "text" | "reasoning" | "tool_name" | "tool_arguments" | "unknown";
  value: string;
}

export interface ProxyStreamReconstruction {
  fragments: ProxyDerivedFragment[];
  combinedText: string;
  combinedReasoning: string;
  combinedToolArguments: string;
}

export interface ProxyRequestSession {
  requestId: string;
  requestStart?: ProxyRequestStartRecord;
  requestEnd?: ProxyRequestEndRecord;
  streamEvents: ProxyStreamEventRecord[];
  requestKind: string;
  isStreaming: boolean;
  reconstruction: ProxyStreamReconstruction;
}

export interface ParsedProxyLog {
  sessions: ProxyRequestSession[];
  issues: ProxyParseIssue[];
}

export function parseProxyNdjson(contents: string): ParsedProxyLog {
  const grouped = new Map<string, ProxyRequestSession>();
  const issues: ProxyParseIssue[] = [];

  for (const [index, rawLine] of contents.split("\n").entries()) {
    const lineNumber = index + 1;
    const line = rawLine.trim();
    if (line.length === 0) {
      continue;
    }

    let value: unknown;
    try {
      value = JSON.parse(line);
    } catch (error) {
      issues.push({
        lineNumber,
        message: error instanceof Error ? error.message : "Invalid JSON",
        rawLine,
      });
      continue;
    }

    if (!isProxyRecord(value)) {
      issues.push({
        lineNumber,
        message: "Record is missing string fields 'record_type' and 'request_id'",
        rawLine,
      });
      continue;
    }

    const session = grouped.get(value.request_id) ?? createEmptySession(value.request_id);
    attachRecord(session, value);
    grouped.set(value.request_id, session);
  }

  const sessions = Array.from(grouped.values())
    .map((session) => ({
      ...session,
      streamEvents: [...session.streamEvents].sort(compareStreamEvents),
      reconstruction: reconstructStream(session.streamEvents),
    }))
    .sort(compareSessions);

  return { sessions, issues };
}

export function sessionError(session: ProxyRequestSession): RunError | null {
  const message = session.requestEnd?.error;
  if (!message) {
    return null;
  }

  return {
    kind: "proxy_request_failed",
    message,
  };
}

function createEmptySession(requestId: string): ProxyRequestSession {
  return {
    requestId,
    streamEvents: [],
    requestKind: "unknown",
    isStreaming: false,
    reconstruction: {
      fragments: [],
      combinedText: "",
      combinedReasoning: "",
      combinedToolArguments: "",
    },
  };
}

function attachRecord(session: ProxyRequestSession, record: ProxyLogRecord) {
  if (isRequestStartRecord(record)) {
    session.requestStart = record;
    session.requestKind = record.kind ?? session.requestKind;
    session.isStreaming =
      session.isStreaming || isStreamingRequestBody(record.request_body) || session.streamEvents.length > 0;
    return;
  }

  if (isRequestEndRecord(record)) {
    session.requestEnd = record;
    session.requestKind = record.kind ?? session.requestKind;
    return;
  }

  if (isStreamEventRecord(record)) {
    session.streamEvents.push(record);
    session.isStreaming = true;
  }
}

function compareSessions(left: ProxyRequestSession, right: ProxyRequestSession): number {
  const leftTimestamp = left.requestStart?.started_at ?? left.streamEvents[0]?.received_at ?? "";
  const rightTimestamp = right.requestStart?.started_at ?? right.streamEvents[0]?.received_at ?? "";
  if (leftTimestamp !== rightTimestamp) {
    return leftTimestamp.localeCompare(rightTimestamp);
  }
  return left.requestId.localeCompare(right.requestId);
}

function compareStreamEvents(left: ProxyStreamEventRecord, right: ProxyStreamEventRecord): number {
  return (left.received_at ?? "").localeCompare(right.received_at ?? "");
}

function isProxyRecord(value: unknown): value is ProxyLogRecord {
  if (typeof value !== "object" || value === null) {
    return false;
  }

  const candidate = value as Record<string, unknown>;
  return typeof candidate.record_type === "string" && typeof candidate.request_id === "string";
}

function isRequestStartRecord(record: ProxyLogRecord): record is ProxyRequestStartRecord {
  return record.record_type === "request_start";
}

function isRequestEndRecord(record: ProxyLogRecord): record is ProxyRequestEndRecord {
  return record.record_type === "request_end";
}

function isStreamEventRecord(record: ProxyLogRecord): record is ProxyStreamEventRecord {
  return record.record_type === "stream_event";
}

function isStreamingRequestBody(requestBody: JsonValue | undefined): boolean {
  return (
    typeof requestBody === "object" &&
    requestBody !== null &&
    !Array.isArray(requestBody) &&
    requestBody.stream === true
  );
}

function reconstructStream(streamEvents: ProxyStreamEventRecord[]): ProxyStreamReconstruction {
  const fragments = streamEvents.flatMap(deriveFragments);

  return {
    fragments,
    combinedText: fragments
      .filter((fragment) => fragment.kind === "text")
      .map((fragment) => fragment.value)
      .join(""),
    combinedReasoning: fragments
      .filter((fragment) => fragment.kind === "reasoning")
      .map((fragment) => fragment.value)
      .join(""),
    combinedToolArguments: fragments
      .filter((fragment) => fragment.kind === "tool_arguments")
      .map((fragment) => fragment.value)
      .join(""),
  };
}

function deriveFragments(record: ProxyStreamEventRecord): ProxyDerivedFragment[] {
  if (!record.data_raw) {
    return [];
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(record.data_raw);
  } catch {
    return [{ kind: "unknown", value: record.data_raw }];
  }

  const fragments: ProxyDerivedFragment[] = [];
  collectFragments(parsed, fragments);
  return fragments.length > 0 ? fragments : [{ kind: "unknown", value: record.data_raw }];
}

function collectFragments(value: unknown, fragments: ProxyDerivedFragment[]) {
  if (typeof value !== "object" || value === null) {
    return;
  }

  const record = value as Record<string, unknown>;

  if (typeof record.delta === "string") {
    fragments.push({ kind: "text", value: record.delta });
  }
  if (typeof record.text === "string") {
    fragments.push({ kind: "text", value: record.text });
  }
  if (typeof record.reasoning_content === "string") {
    fragments.push({ kind: "reasoning", value: record.reasoning_content });
  }
  if (typeof record.thinking === "string") {
    fragments.push({ kind: "reasoning", value: record.thinking });
  }
  if (typeof record.name === "string" && looksLikeToolNameContainer(record)) {
    fragments.push({ kind: "tool_name", value: record.name });
  }
  if (typeof record.arguments === "string") {
    fragments.push({ kind: "tool_arguments", value: record.arguments });
  }

  for (const child of Object.values(record)) {
    if (Array.isArray(child)) {
      for (const item of child) {
        collectFragments(item, fragments);
      }
      continue;
    }
    collectFragments(child, fragments);
  }
}

function looksLikeToolNameContainer(record: Record<string, unknown>): boolean {
  return "arguments" in record || "tool_calls" in record || "function" in record;
}
