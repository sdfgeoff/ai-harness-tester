FROM debian:bookworm-slim

COPY run.sh /usr/local/bin/run-smoke-harness
RUN chmod +x /usr/local/bin/run-smoke-harness

ENTRYPOINT ["/usr/local/bin/run-smoke-harness"]
