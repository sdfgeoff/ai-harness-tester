FROM node:26-bookworm

# Install tool as root
RUN npm install -g opencode-ai

# Copy and prepare entrypoint as root
COPY run.sh /usr/local/bin/run
RUN chmod +x /usr/local/bin/run && \
    mkdir -p /home/node/.config/opencode

# Switch to non-root user
USER node
WORKDIR /workdir

ENTRYPOINT ["/usr/local/bin/run"]
