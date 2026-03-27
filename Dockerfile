FROM rust:1.85-bookworm AS builder

WORKDIR /usr/src/sitrep

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash sitrep
USER sitrep

COPY --from=builder /usr/src/sitrep/target/release/sitrep /usr/local/bin/sitrep

ENV TERM=xterm-256color
ENV RUST_BACKTRACE=1

CMD ["sitrep"]
