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

## Bitfinity commands

### Running the node as EVM archiver

When running the node, it will also run a block import process. This process will download the blocks from the network and import them into the database. This process can take a long time, depending on the network speed and the number of blocks to import.

To run the node, use the following command:

```sh
reth node -vvv --http --http.port 8080 --http.addr 0.0.0.0 --http.api "debug,eth,net,trace,txpool,web3" --disable-discovery --ipcdisable --no-persist-peers -r https://testnet.bitfinity.network -i 30 -b 100 --datadir /reth/data
```


With cargo: 

```sh
cargo run -p reth -- node -vvv --http --http.port 8080 --http.addr 0.0.0.0 --http.api "debug,eth,net,trace,txpool,web3" --disable-discovery --ipcdisable --no-persist-peers -r https://orca-app-5yyst.ondigitalocean.app -i 30 -b 100 --max-fetch-blocks 5000 --datadir ./target/reth
```

You can query the node using the JSON-RPC API. For example, to get the block number, you can use the following command:

```sh
curl -X POST -H 'content-Type: application/json' --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' http://localhost:8080
```

#### Optional arguments

- `send-raw-transaction-rpc-url`: If provided, every `eth_sendRawTransaction` will be forwarded to this URL. When not provided, the `rpc_url` will be used instead as forwarding URL.


#### Running the node with Docker

You can locally build the docker image by using the provided `docker-compose.yml` file
```sh
docker compose build
```

Then to start it:
```sh
docker run -d -p 8080:8080 bitfinity/reth node --http.port 8080 --http.addr 0.0.0.0 --http.api "debug,eth,net,trace,txpool,web3" --disable-discovery --ipcdisable --no-persist-peers -r https://testnet.bitfinity.network -i 15 -b 1000
```

Alternatively, you can use the prebuilt Docker image from `ghcr.io`:
```sh
docker run ghcr.io/bitfinity-network/bitfinity-reth:main node --http.port 8080 --http.addr 0.0.0.0 --http.api "debug,eth,net,trace,txpool,web3" --disable-discovery --ipcdisable --no-persist-peers -r https://testnet.bitfinity.network
```


### Reset the EVM canister state

You can use the current node state to reset the state of an EVM canister. By the end of this process, the EVM canister will have the same state root and last block as the Reth node.

*Preconditions*: The IC account must have admin rights on the EVM canister.

To execute the reset command, follow these steps:

1. Shut down the reth node.

2. Disable the EVM canister:
```sh
dfx canister --network=ic call EVM_CANISTER_ID admin_disable_evm '(true)'
```

3. Run the EVM reset command. For example:
```sh
cargo run -p reth -- bitfinity-reset-evm-state -vvv --datadir ./target/reth --ic-identity-file-path PATH_TO_IDENTITY/identity.pem --evm-network https://ic0.app --evmc-principal EVM_CANISTER_ID --evm-datasource-url https://orca-app-5yyst.ondigitalocean.app
```

4. Enable the EVM canister:
```sh
dfx canister --network=ic call EVM_CANISTER_ID admin_disable_evm '(false)'
```
