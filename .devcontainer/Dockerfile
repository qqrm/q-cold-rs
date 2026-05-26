FROM mcr.microsoft.com/devcontainers/rust:1-1-bookworm AS devcontainer-slim

FROM devcontainer-slim AS devcontainer-e2e

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        jq \
        openssh-client \
        p7zip-full \
        rsync \
        sqlite3 \
    && rm -rf /var/lib/apt/lists/*
