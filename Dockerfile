FROM rust:1.90-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock dist-workspace.toml ./
COPY crates/ crates/

RUN cargo build --release --bin claude-pool-server

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install Claude Code CLI
RUN curl -fsSL https://claude.ai/install.sh | sh

COPY --from=builder /build/target/release/claude-pool-server /usr/local/bin/claude-pool-server

ENTRYPOINT ["claude-pool-server"]
CMD ["-n", "2"]
