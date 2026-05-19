FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Install opencode via official install script
RUN curl -fsSL https://opencode.ai/install.sh | sh

COPY run.sh /usr/local/bin/run-opencode-harness
RUN chmod +x /usr/local/bin/run-opencode-harness

ENTRYPOINT ["/usr/local/bin/run-opencode-harness"]
