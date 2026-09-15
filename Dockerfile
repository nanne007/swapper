FROM rust:1.94-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update \
    && apt-get install --no-install-recommends --yes ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/metamatch-backend /usr/local/bin/metamatch-backend
USER nobody:nogroup
EXPOSE 3000
CMD ["metamatch-backend"]
