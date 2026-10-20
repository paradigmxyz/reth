FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends python3 libssl3t64 libstdc++6 && rm -rf /var/lib/apt/lists/*
COPY trie-dst bedrock-ready controller.py /usr/local/bin/
ENTRYPOINT ["/bin/sh", "-c", "/usr/local/bin/bedrock-ready && exec sleep infinity"]
