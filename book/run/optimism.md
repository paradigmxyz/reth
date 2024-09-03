# Running Reth on OP Stack chains

`reth` ships with the `optimism` feature flag in several crates, including the binary, enabling support for OP Stack chains out of the box. Optimism has a small diff from the [L1 EELS][l1-el-spec],
comprising of the following key changes:
1. A new transaction type, [`0x7E (Deposit)`][deposit-spec], which is used to deposit funds from L1 to L2.
1. Modifications to the `PayloadAttributes` that allow the [sequencer][sequencer] to submit transactions to the EL through the Engine API. Payloads will be built with deposit transactions at the top of the block,
   with the first deposit transaction always being the "L1 Info Transaction."
1. EIP-1559 denominator and elasticity parameters have been adjusted to account for the lower block time (2s) on L2. Otherwise, the 1559 formula remains the same.
1. Network fees are distributed to the various [fee vaults][l2-el-spec].
1. ... and some other minor changes.

For a more in-depth list of changes and their rationale, as well as specifics about the OP Stack specification such as transaction ordering and more, see the documented [`op-geth` diff][op-geth-forkdiff],
the [L2 EL specification][l2-el-spec], and the [OP Stack specification][op-stack-spec].

## Running on Optimism

You will need three things to run `op-reth`:
1. An archival L1 node, synced to the settlement layer of the OP Stack chain you want to sync (e.g. `reth`, `geth`, `besu`, `nethermind`, etc.)
1. A rollup node (e.g. `op-node`, `magi`, `hildr`, etc.)
1. An instance of `op-reth`.

For this example, we'll start a `Base Mainnet` node.

### Installing `op-reth`

To run Reth on Optimism, first install `op-reth` via the `Makefile` in the workspace root:

```sh
git clone https://github.com/paradigmxyz/reth.git && \
    cd reth && \
    make install-op
```

This will install the `op-reth` binary to `~/.cargo/bin/op-reth`.

### Installing a Rollup Node

Next, you'll need to install a [Rollup Node][rollup-node-spec], which is the equivalent to the Consensus Client on the OP Stack. Available options include:
1. [`op-node`][op-node]
1. [`magi`][magi]
1. [`hildr`][hildr]

For the sake of this tutorial, we'll use the reference implementation of the Rollup Node maintained by OP Labs, the `op-node`. The `op-node` can be built from source, or pulled from a [Docker image available on Google Cloud][op-node-docker].

**`rethdb` build tag**  
The `op-node` also comes with an experimental `rethdb` build tag, which allows it to read receipts directly from an L1 `reth` database during [derivation][derivation-spec]. This can speed up sync times, but it is not required if you do not
have access to the L1 archive node on the same machine as your L2 node.

To build the `op-node` with the `rethdb` build tag enabled:
```sh
git clone https://github.com/ethereum-optimism/optimism.git && \
    (cd optimism/op-service/rethdb-reader && cargo build --release) && \ 
    cd optimism/op-node && \
    go build -v -tags rethdb -o ./bin/op-node ./cmd/main.go && \
    mv bin/op-node /usr/bin/op-node
```
This will build the `rethdb-reader` dylib and instruct the `op-node` build to statically link this dylib into the binary. The `op-node` binary will be installed to `/usr/bin/op-node`.

### Running `op-reth`

The `optimism` feature flag in `op-reth` adds several new CLI flags to the `reth` binary:
1. `--rollup.sequencer-http <uri>` - The sequencer endpoint to connect to. Transactions sent to the `op-reth` EL are also forwarded to this sequencer endpoint for inclusion, as the sequencer is the entity that builds blocks on OP Stack chains.
1. `--rollup.disable-tx-pool-gossip` - Disables gossiping of transactions in the mempool to peers. This can be omitted for personal nodes, though providers should always opt to enable this flag.
1. `--rollup.enable-genesis-walkback` - Disables setting the forkchoice status to tip on startup, making the `op-node` walk back to genesis and verify the integrity of the chain before starting to sync. This can be omitted unless a corruption of local chainstate is suspected.
1. `--rollup.discovery.v4` - Enables the discovery v4 protocol for peer discovery. By default, op-reth, similar to op-geth, has discovery v5 enabled and discovery v4 disabled, whereas regular reth has discovery v4 enabled and discovery v5 disabled.

First, ensure that your L1 archival node is running and synced to tip. Also make sure that the beacon node / consensus layer client is running and has http APIs enabled. Then, start `op-reth` with the `--rollup.sequencer-http` flag set to the `Base Mainnet` sequencer endpoint:
```sh
op-reth node \
    --chain base \
    --rollup.sequencer-http https://mainnet-sequencer.base.org \
    --http \
    --ws \
    --authrpc.port 9551 \
    --authrpc.jwtsecret /path/to/jwt.hex
```

Then, once `op-reth` has been started, start up the `op-node`:
```sh
op-node \
    --network="base-mainnet" \
    --l1=<your-L1-rpc> \
    --l2=http://localhost:9551 \
    --l2.jwt-secret=/path/to/jwt.hex \
    --rpc.addr=0.0.0.0 \
    --rpc.port=7000 \
    --l1.beacon=<your-beacon-node-http-endpoint>
    --syncmode=execution-layer
    --l2.enginekind=reth
```

Consider adding the `--l1.trustrpc` flag to improve performance, if the connection to l1 is over localhost.

If you opted to build the `op-node` with the `rethdb` build tag, this feature can be enabled by appending one extra flag to the `op-node` invocation:

> Note, the `reth_db_path` is the path to the `db` folder inside of the reth datadir, not the `mdbx.dat` file itself. This can be fetched from `op-reth db path [--chain <chain-name>]`, or if you are using a custom datadir location via the `--datadir` flag,
> by appending `/db` to the end of the path.

```sh
op-node \
    # ...
    --l1.rethdb=<your_L1_reth_db_path>
```

[l1-el-spec]: https://github.com/ethereum/execution-specs
[rollup-node-spec]: https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/rollup-node.md
[op-geth-forkdiff]: https://op-geth.optimism.io
[sequencer]: https://github.com/ethereum-optimism/specs/blob/main/specs/background.md#sequencers
[op-stack-spec]: https://github.com/ethereum-optimism/specs/blob/main/specs
[l2-el-spec]: https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/exec-engine.md
[deposit-spec]: https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/deposits.md
[derivation-spec]: https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/derivation.md

[op-node-docker]: https://console.cloud.google.com/artifacts/docker/oplabs-tools-artifacts/us/images/op-node

[reth]: https://github.com/paradigmxyz/reth
[op-node]: https://github.com/ethereum-optimism/optimism/tree/develop/op-node
[magi]: https://github.com/a16z/magi
[hildr]: https://github.com/optimism-java/hildr
