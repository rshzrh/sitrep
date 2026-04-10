FROM rust:1.85-bookworm AS builder

WORKDIR /usr/src/flotop

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    openssh-client \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash flotop
USER flotop

COPY --from=builder /usr/src/flotop/target/release/flotop /usr/local/bin/flotop

ENV TERM=xterm-256color
ENV RUST_BACKTRACE=1

CMD ["flotop"]
