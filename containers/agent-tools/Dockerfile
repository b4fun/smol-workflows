# syntax=docker/dockerfile:1

# Agent tool image for sandbox providers.
#
# This image is intended for Azure Sandbox OCI-image profiles. It includes the
# built-in CLI agent providers that smol-workflows can execute inside a sandbox,
# plus common development tools needed for git workspace sync and provider helper
# scripts.

FROM node:24-bookworm-slim

ARG PI_VERSION=0.79.3
ARG OPENCODE_VERSION=1.17.7
ARG CLAUDE_CODE_VERSION=2.1.177
ARG CODEX_VERSION=0.139.0

LABEL org.opencontainers.image.title="smol-workflows agent tools"
LABEL org.opencontainers.image.description="CLI agent provider tools for smol-workflows sandbox execution"
LABEL org.opencontainers.image.source="https://github.com/b4fun/smol-workflows"

ENV DEBIAN_FRONTEND=noninteractive \
    NODE_ENV=production \
    NPM_CONFIG_UPDATE_NOTIFIER=false \
    NPM_CONFIG_FUND=false \
    NPM_CONFIG_AUDIT=false

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        coreutils \
        curl \
        file \
        findutils \
        git \
        git-lfs \
        gnupg \
        grep \
        gzip \
        jq \
        less \
        openssh-client \
        procps \
        python3 \
        python3-pip \
        ripgrep \
        sed \
        tar \
        unzip \
        xz-utils \
        zip \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g \
        "@earendil-works/pi-coding-agent@${PI_VERSION}" \
        "opencode-ai@${OPENCODE_VERSION}" \
        "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
        "@openai/codex@${CODEX_VERSION}" \
    && npm cache clean --force

RUN command -v git \
    && command -v python3 \
    && command -v curl \
    && command -v jq \
    && command -v rg \
    && command -v pi \
    && command -v opencode \
    && command -v claude \
    && command -v codex \
    && pi --version \
    && opencode --version \
    && claude --version \
    && codex --version

WORKDIR /workspace

CMD ["bash"]
