FROM python:3.12-slim

COPY evaluate.py /usr/local/bin/evaluate
RUN chmod +x /usr/local/bin/evaluate

ENTRYPOINT ["/usr/local/bin/evaluate"]
