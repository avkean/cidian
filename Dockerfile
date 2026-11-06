# the frontend and public English inputs are compiled in
# rust:1-slim-bookworm
FROM rust@sha256:24707e70a4f1e23d9a7b25202ec43126d28a52294b96aad5c670df1fd1f1d400 AS build
WORKDIR /app
# A stub build lets source-only edits reuse compiled dependencies.
# The registry and target caches outlive the build.
COPY Cargo.toml Cargo.lock ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target,sharing=locked \
    mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release --locked -j2 --bin cidian && rm -r src
COPY data/required-notices.json data/traditional-kept.tsv data/dictionary.jsonl data/english-support.tsv data/characters.jsonl data/
COPY data/english-lexicon.tsv data/english-usage.tsv data/english-words.txt data/english-stopwords.tsv data/
COPY data/licenses data/licenses
COPY src src
COPY web web
# touched so the crate always rebuilds over the cached stub; target is a cache, so the binary is copied out
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target,sharing=locked \
    touch src/main.rs && cargo build --release --locked -j2 --bin cidian && cp target/release/cidian /app/cidian-bin

# just the binary and the data it reads, as non-root
# gcr.io/distroless/cc-debian12:nonroot
FROM gcr.io/distroless/cc-debian12@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
WORKDIR /app
COPY --from=build /app/cidian-bin /app/cidian
COPY --from=build /app/data/dictionary.jsonl /app/data/characters.jsonl /app/data/english-support.tsv /app/data/
COPY --from=build /app/data/licenses /app/data/licenses
COPY LICENSE /app/LICENSE
ENV CIDIAN_HOST=0.0.0.0 \
    CIDIAN_PORT=3000 \
    CIDIAN_DATA=/app/data
EXPOSE 3000
# no shell or curl in the image, so the binary probes its own /api/health
HEALTHCHECK --interval=30s --timeout=5s --start-period=60s --start-interval=2s --retries=3 CMD ["/app/cidian", "--health"]
ENTRYPOINT ["/app/cidian"]
