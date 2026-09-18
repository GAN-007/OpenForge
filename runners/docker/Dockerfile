FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    git \
    build-essential \
    pkg-config \
    libssl-dev \
    python3 \
    python3-pip \
    python3-venv \
    nodejs \
    npm \
    cargo \
    rustc \
    jq \
    ripgrep \
    && npm install -g pnpm@10.15.1 \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 --shell /bin/bash openforge

USER openforge
WORKDIR /workspace
ENV HOME=/home/openforge

ENTRYPOINT []
CMD ["bash"]
