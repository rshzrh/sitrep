FROM rust:latest

WORKDIR /usr/src/sitrep

# Copy manifest and source
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build release binary
RUN cargo build --release

# Set terminal environment for crossterm
ENV TERM=xterm-256color

# Run the binary
CMD ["./target/release/sitrep"]
