# Build context is the repository root:
#   docker compose -f scripts/omniworld-server/docker-compose.yml build
FROM rust:1-bookworm AS builder
WORKDIR /build
# Only the server crate: the workspace root is the Tauri app, with heavy system deps.
COPY src-tauri/omniworld-server/ ./
# Detach the crate from the workspace so it builds standalone.
RUN printf '\n[workspace]\n' >> Cargo.toml
RUN cargo build --release

FROM debian:bookworm-slim
RUN useradd --system --no-create-home --uid 10001 omniworld
COPY --from=builder /build/target/release/omniworld-server /usr/local/bin/omniworld-server
USER omniworld
EXPOSE 7878
ENTRYPOINT ["/usr/local/bin/omniworld-server"]
