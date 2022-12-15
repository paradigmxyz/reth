FROM rust:1.65 AS chef

RUN apt-get update && apt-get install -y libclang-dev

RUN cargo install cargo-chef


FROM chef AS planner

WORKDIR /reth-planner

COPY . .

RUN cargo chef prepare --recipe-path recipe.json


FROM chef AS builder

WORKDIR /reth-builder

COPY --from=planner /reth-planner/recipe.json recipe.json

RUN cargo chef cook --recipe-path recipe.json

COPY . .

RUN cargo build


FROM debian:11-slim

WORKDIR /reth

COPY --from=builder /reth-builder/target/debug .

ENTRYPOINT [ "./reth" ]
CMD [ "help" ]
