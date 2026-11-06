FROM rust:1.99-slim-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
RUN cargo build --locked --release

FROM debian:bookworm-slim
WORKDIR /app
RUN mkdir /data
COPY --from=build /build/target/release/pig6bot /usr/local/bin/pig6bot
COPY web ./web
COPY assets ./assets
COPY dict.json olymps.json ./
EXPOSE 2322
ENTRYPOINT ["pig6bot"]
CMD ["all", "--data-dir", "/data"]
