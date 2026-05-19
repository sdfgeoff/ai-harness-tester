FROM node:26-bookworm

RUN npm install -g @earendil-works/pi-coding-agent

COPY run.sh /usr/local/bin/run
RUN chmod +x /usr/local/bin/run

ENTRYPOINT ["/usr/local/bin/run"]