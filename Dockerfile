# syntax=docker/dockerfile:1
FROM rust:1.86-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY docs ./docs
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/* \
  && useradd --system --home /data --shell /usr/sbin/nologin nexuspouch
COPY --from=build /src/target/release/nexuspouch /usr/local/bin/nexuspouch
USER nexuspouch
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8787
ENV RUST_LOG=info
ENTRYPOINT ["/usr/local/bin/nexuspouch"]
CMD ["--root", "/data", "--listen", ":8787", "--name", "nexuspouch"]
