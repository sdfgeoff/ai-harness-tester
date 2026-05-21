FROM debian:bookworm-slim

COPY evaluate.sh /usr/local/bin/evaluate
RUN chmod +x /usr/local/bin/evaluate

ENTRYPOINT ["/usr/local/bin/evaluate"]
