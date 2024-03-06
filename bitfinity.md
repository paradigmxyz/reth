# Bitfinity Reth Node

## Introduction

Bitfinity Reth is an archive node built on Reth, a rust implementation of the Ethereum protocol. It is designed to be fast, efficient, and secure.

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install)
- [Git](https://git-scm.com/book/en/v2/Getting-Started-Installing-Git)
- [CMake](https://cmake.org/install/)

### Building from Source

```sh
make build
```

### Running the node

When running the node, it will also run a block import process. This process will download the blocks from the network and import them into the database. This process can take a long time, depending on the network speed and the number of blocks to import.

Before running the node, you will need to have a `bitfinity.spec.json` file in the root of the project. This file should contain the genesis block and other configuration options for the node. An example of this file can be found in the root of the project.

To run the node, use the following command:

```sh
reth node -vvvvv --chain bitfinity.spec.json --http --http.port 8080 -d -r https://testnet.bitfinity.network -i 30 -b 100 --datadir ./target/reth
```


With cargo: 

```sh
cargo run -p reth -- node -vvvv --chain bitfinity.spec.json --http --http.port 8080 -d -r https://orca-app-5yyst.ondigitalocean.app -i 30 -b 100  --datadir ./target/reth
```


### Querying the node

You can query the node using the JSON-RPC API. For example, to get the block number, you can use the following command:

```sh
curl -X POST -H 'content-Type: application/json' --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' http://localhost:8080
```

### Running the node with Docker

### Docker Image

To build the docker image, use the following command:

```sh
make docker
```

### To run the docker image

```sh
docker run -d -p 8080:8080 bitfinity/reth node --chain bitfinity.spec.json --http --http.port 8080 -d -r https://testnet.bitfinity.network -i 30 -b 10
```

### To run pre-built docker image

```sh
docker run ghcr.io/bitfinity-network/bitfinity-reth:main node --chain bitfinity.spec.json --http --http.port 8080 -d -r https://testnet.bitfinity.network -i 30 -b 10
```
