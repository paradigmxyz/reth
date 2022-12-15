FROM rust:1.65 AS builder

WORKDIR /build

RUN apt-get update && apt-get install -y libclang-dev

COPY . .

RUN cargo build


FROM debian:11-slim

WORKDIR /reth

COPY --from=builder /build/target/debug .

ENTRYPOINT [ "./reth" ]
CMD [ "help" ]
