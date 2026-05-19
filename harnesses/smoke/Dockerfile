FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends tini && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN groupadd -g 1000 agent && \
    useradd -u 1000 -g 1000 -m -s /bin/sh agent

# Copy and prepare entrypoint as root
COPY run.sh /usr/local/bin/run-smoke-harness
RUN chmod +x /usr/local/bin/run-smoke-harness

# Switch to non-root user
USER agent
WORKDIR /workdir

ENTRYPOINT ["/usr/local/bin/run-smoke-harness"]
