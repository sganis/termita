# Dockerfile
# Three stages: build the Svelte bundle (bun) -> compile a static musl binary that
# embeds that bundle (cargo + ring) -> ship just the binary on `scratch`.
# No Node, no Bun, no node_modules, no ssh client, no /etc/passwd hack at runtime.

# 1) Frontend -> /web/dist
FROM oven/bun:1 AS web
WORKDIR /web
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile
COPY web/ ./
RUN bun run build

# 2) Static Rust binary. Alpine's default Rust target is x86_64-unknown-linux-musl,
#    so `cargo build` already yields a fully static binary. rust-embed bakes
#    /web/dist into the binary at compile time.
FROM rust:1-alpine AS server
RUN apk add --no-cache build-base
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY --from=web /web/dist ./web/dist
RUN cargo build --release

# 3) Runtime: nothing but the binary. Runs as a non-root, arbitrary UID — russh
#    never calls getpwuid, so OpenShift's random UID is fine (no passwd entry needed).
FROM scratch
COPY --from=server /src/target/release/termita /termita
ENV PORT=3000 \
    HOST=0.0.0.0
EXPOSE 3000
USER 1001
ENTRYPOINT ["/termita"]
