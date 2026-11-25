FROM rust:1.100.0-alpine3.24@sha256:8d0e82abfdf9a38071e07cabc5afba4ddbe077631a80e4e742a760c38beadc33 AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM scratch
COPY --from=builder /app/target/release/roalt-remote-relay /roalt-remote-relay
USER 10001:10001
ENV RELAY_BIND=0.0.0.0:8788
EXPOSE 8788
ENTRYPOINT ["/roalt-remote-relay"]
