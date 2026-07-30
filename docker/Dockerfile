FROM rust:slim-bookworm AS builder

ARG SERVICE_NAME

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY services/ ./

RUN cargo build --release --bin ${SERVICE_NAME}

FROM debian:bookworm-slim

ARG SERVICE_NAME

RUN apt-get update && apt-get install -y ca-certificates ffmpeg && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/${SERVICE_NAME} /usr/local/bin/service

ENV PORT=3000
EXPOSE 3000

CMD ["service"]
