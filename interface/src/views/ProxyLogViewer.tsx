import { useState } from "react";
import { fetchProxyLog } from "../data";
import { formatDuration, formatTimestamp } from "../format";
import {
  sessionError,
  type ParsedProxyLog,
  type ProxyRequestSession,
  type ProxyToolCall,
  type ProxyUsage,
} from "../proxy";

interface ProxyLogViewerProps {
  batchId: string;
  resultsPath: string;
  proxyLogPath: string;
}

export function ProxyLogViewer({ batchId, resultsPath, proxyLogPath }: ProxyLogViewerProps) {
  const [parsedLog, setParsedLog] = useState<ParsedProxyLog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);

  async function loadProxyLog() {
    if (parsedLog || isLoading) {
      return;
    }

    setIsLoading(true);
    setError(null);
    try {
      const log = await fetchProxyLog(batchId, resultsPath, proxyLogPath);
      setParsedLog(log);
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : "Failed to load proxy log");
    } finally {
      setIsLoading(false);
    }
  }

  return (
    <section className="subpanel">
      <div className="panel-header">
        <h3>Proxy Requests</h3>
        {!parsedLog && !isLoading ? (
          <button className="secondary-button" type="button" onClick={loadProxyLog}>
            Load proxy.ndjson
          </button>
        ) : null}
      </div>

      {isLoading ? <p className="muted-text">Loading proxy log…</p> : null}
      {error ? <p className="error-text">{error}</p> : null}
      {parsedLog ? (
        <>
          {parsedLog.issues.length > 0 ? (
            <div className="warning-block">
              <strong>Parse issues</strong>
              <ul className="issue-list">
                {parsedLog.issues.map((issue) => (
                  <li key={`${issue.lineNumber}-${issue.message}`}>
                    <span>
                      line {issue.lineNumber}: {issue.message}
                    </span>
                    <code>{issue.rawLine}</code>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}

          <div className="proxy-session-list">
            {parsedLog.sessions.map((session) => (
              <ProxySessionCard key={session.requestId} session={session} />
            ))}
          </div>
        </>
      ) : null}
    </section>
  );
}

function ProxySessionCard({ session }: { session: ProxyRequestSession }) {
  const start = session.requestStart;
  const end = session.requestEnd;
  const error = sessionError(session);
  const requestBody = start?.request_body;
  const responseBody = end?.response_body;
  const fragmentCount = session.reconstruction.fragments.length;

  return (
    <details className="proxy-session-card">
      <summary className="proxy-session-summary">
        <div className="proxy-session-heading">
          <span className="status-pill">{session.requestKind}</span>
          {session.isStreaming ? <span className="status-pill is-streaming">streaming</span> : null}
          {typeof end?.status_code === "number" ? (
            <span className="status-pill">{end.status_code}</span>
          ) : null}
          {error ? <span className="status-pill is-error">error</span> : null}
        </div>
        <div className="proxy-session-title">
          <strong>{start?.method ?? end?.method ?? "?"}</strong>
          <code>{start?.path ?? end?.path ?? "unknown path"}</code>
        </div>
        <div className="proxy-session-meta">
          <span>{formatTimestamp(start?.started_at)}</span>
          <span>{formatDuration(end?.duration_ms)}</span>
          <span>{start?.upstream_model ?? end?.upstream_model ?? "unknown model"}</span>
        </div>
      </summary>

      <div className="proxy-session-body">
        <dl className="kv-grid">
          <KV label="Request ID" value={session.requestId} />
          <KV label="Original model" value={start?.original_model ?? end?.original_model ?? "—"} />
          <KV label="Upstream model" value={start?.upstream_model ?? end?.upstream_model ?? "—"} />
          <KV label="Started" value={formatTimestamp(start?.started_at)} />
          <KV label="Finished" value={formatTimestamp(end?.finished_at)} />
          <KV label="Duration" value={formatDuration(end?.duration_ms)} />
          <KV label="Stream events" value={String(session.streamEvents.length)} />
          <KV label="Derived fragments" value={String(fragmentCount)} />
        </dl>

        <section className="subpanel">
          <h4>Usage</h4>
          <dl className="kv-grid">
            {formatUsage(end?.usage).map(([label, value]) => (
              <KV key={label} label={label} value={value} />
            ))}
          </dl>
        </section>

        {session.reconstruction.combinedText ? (
          <section className="subpanel">
            <h4>Reconstructed text</h4>
            <pre className="code-block">{session.reconstruction.combinedText}</pre>
          </section>
        ) : null}

        {session.reconstruction.combinedReasoning ? (
          <section className="subpanel">
            <h4>Reconstructed reasoning</h4>
            <pre className="code-block">{session.reconstruction.combinedReasoning}</pre>
          </section>
        ) : null}

        {session.reconstruction.toolCalls.length > 0 ? (
          <section className="subpanel">
            <h4>Reconstructed tool calls</h4>
            <div className="tool-call-list">
              {session.reconstruction.toolCalls.map((toolCall) => (
                <ToolCallCard key={toolCall.key} toolCall={toolCall} />
              ))}
            </div>
          </section>
        ) : null}

        {error ? (
          <div className="error-block">
            <strong>{error.kind}</strong>
            <p>{error.message}</p>
          </div>
        ) : null}

        <details className="raw-block">
          <summary>Request body</summary>
          <pre className="code-block">{formatJson(requestBody)}</pre>
        </details>

        <details className="raw-block">
          <summary>Response body</summary>
          <pre className="code-block">{formatJson(responseBody)}</pre>
        </details>

        {session.streamEvents.length > 0 ? (
          <details className="raw-block">
            <summary>Stream events</summary>
            <div className="stream-event-list">
              {session.streamEvents.map((event, index) => (
                <div key={`${event.request_id}-${event.received_at ?? index}`} className="stream-event-card">
                  <div className="stream-event-meta">
                    <span>{formatTimestamp(event.received_at)}</span>
                    <span>{event.event || "message"}</span>
                  </div>
                  <pre className="code-block">{event.data_raw ?? ""}</pre>
                </div>
              ))}
            </div>
          </details>
        ) : null}
      </div>
    </details>
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

function ToolCallCard({ toolCall }: { toolCall: ProxyToolCall }) {
  return (
    <div className="tool-call-card">
      <dl className="kv-grid">
        <KV label="Name" value={toolCall.name ?? "unknown"} />
        <KV label="Call ID" value={toolCall.id ?? "—"} />
        <KV label="Index" value={toolCall.index === undefined ? "—" : String(toolCall.index)} />
      </dl>
      <pre className="code-block">{formatToolArguments(toolCall.argumentsText)}</pre>
    </div>
  );
}

function formatUsage(usage: ProxyUsage | null | undefined): Array<[string, string]> {
  return [
    ["input_tokens", formatNumber(usage?.input_tokens)],
    ["output_tokens", formatNumber(usage?.output_tokens)],
    ["total_tokens", formatNumber(usage?.total_tokens)],
    ["cache_read_tokens", formatNumber(usage?.cache_read_tokens)],
    ["cache_write_tokens", formatNumber(usage?.cache_write_tokens)],
  ];
}

function formatNumber(value: number | null | undefined): string {
  return value === null || value === undefined ? "null" : String(value);
}

function formatJson(value: unknown): string {
  if (value === undefined) {
    return "undefined";
  }
  if (value === null) {
    return "null";
  }

  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function formatToolArguments(value: string): string {
  if (value.length === 0) {
    return "";
  }

  try {
    return JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    return value;
  }
}
