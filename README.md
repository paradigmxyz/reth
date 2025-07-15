# reth

[![bench status](https://github.com/paradigmxyz/reth/actions/workflows/bench.yml/badge.svg)](https://github.com/paradigmxyz/reth/actions/workflows/bench.yml)
[![CI status](https://github.com/paradigmxyz/reth/workflows/unit/badge.svg)][gh-ci]
[![cargo-lint status](https://github.com/paradigmxyz/reth/actions/workflows/lint.yml/badge.svg)][gh-lint]
[![Telegram Chat][tg-badge]][tg-url]

```
reth node --engine.accept-execution-requests-hash --datadir ./data \
  --chain dev \
  --authrpc.jwtsecret ./jwt.hex \
  --authrpc.addr 127.0.0.1 \
  --authrpc.port 8551 \
  --http \
  --ws \
  --rpc-max-connections 429496729 \
  --http.api txpool,trace,web3,eth,debug \
  --ws.api trace,web3,eth,debug
  ```