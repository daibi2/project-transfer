# Rust shellctl sandbox image — provides a sandboxed runtime environment
# with common dev tools (Node.js, Python, pnpm, uv) for agent-managed jobs.
#
# Build-time C: rusqlite and zstd vendor C sources and compile them into the
# binaries (equivalent of losing Go's CGO_ENABLED=0). The production stage
# still copies fully-linked ELF files; no libsqlite/libzstd packages are
# required at runtime beyond glibc.
#
# Build:
#   docker build -f docker/Dockerfile -t dify-agent-local-sandbox:local .

# ── Rust build stage ─────────────────────────────────────────────────────────
FROM rust:1.88-bookworm AS rust-builder

WORKDIR /src
RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc libc6-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bins \
    && test -x /src/target/release/shellctl \
    && test -x /src/target/release/shellctl-sanitize-pty \
    && test -x /src/target/release/shellctl-runner-exit \
    && test -x /src/target/release/shellctl-runner \
    && test -x /src/target/release/dify-agent

# ── Runtime stage ────────────────────────────────────────────────────────────
FROM python:3.12-slim-bookworm AS production

ARG NODE_VERSION=24.20.0
ARG PNPM_VERSION=11.9.0
ARG UV_VERSION=0.8.9

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PIP_NO_CACHE_DIR=1

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        file \
        git \
        jq \
        less \
        openssh-client \
        procps \
        ripgrep \
        tini \
        tmux \
        unzip \
        xz-utils \
        zip \
    && node_arch="$(dpkg --print-architecture)" \
    && case "${node_arch}" in \
        amd64) node_arch="x64" ;; \
        arm64) node_arch="arm64" ;; \
        *) echo "Unsupported Node.js architecture: ${node_arch}" >&2; exit 1 ;; \
    esac \
    && node_dist="node-v${NODE_VERSION}-linux-${node_arch}" \
    && curl -fsSLO "https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt" \
    && curl -fsSLO "https://nodejs.org/dist/v${NODE_VERSION}/${node_dist}.tar.xz" \
    && grep " ${node_dist}.tar.xz\$" SHASUMS256.txt | sha256sum -c - \
    && tar -xJf "${node_dist}.tar.xz" -C /usr/local --strip-components=1 \
    && rm -f SHASUMS256.txt "${node_dist}.tar.xz" \
    && npm install --global "pnpm@${PNPM_VERSION}" \
    && npm cache clean --force \
    && rm -rf /var/lib/apt/lists/*

RUN python -m pip install --no-cache-dir "uv==${UV_VERSION}"

COPY --from=rust-builder /src/target/release/shellctl /usr/local/bin/shellctl
COPY --from=rust-builder /src/target/release/shellctl-sanitize-pty /usr/local/bin/shellctl-sanitize-pty
COPY --from=rust-builder /src/target/release/shellctl-runner-exit /usr/local/bin/shellctl-runner-exit
COPY --from=rust-builder /src/target/release/shellctl-runner /usr/local/bin/shellctl-runner
COPY --from=rust-builder /src/target/release/dify-agent /usr/local/bin/dify-agent

RUN useradd --create-home --shell /bin/sh dify \
    && mkdir -p /workspace \
    && chown dify:dify /home \
    && chown -R dify:dify /home/dify /workspace

USER dify
WORKDIR /home/dify

EXPOSE 5004

ENTRYPOINT ["/usr/bin/tini", "-g", "--"]
CMD ["shellctl", "serve", "--listen", "0.0.0.0:5004"]
