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

> `op-reth` is currently in the *alpha* stage of development. It is not yet ready for production use, and therefore does not have a stable release. To run it, you must build the `op-reth` binary from source.
> If you do encounter any bugs during this early stage of development, please report them in an issue on the [GitHub repository][reth].
>
> `op-reth` also does **not** currently support OP Stack chains with legacy, pre-Bedrock state, i.e. `Optimism Mainnet` and `Optimism Goerli`. This will be possible once a database migration tool for pre-Bedrock
> state is released, with the capability to extract the legacy state from the old `l2geth` LevelDB datadir and transplant it into Reth's MDBX database.

You will need three things to run `op-reth`:
1. An archival L1 node, synced to the settlement layer of the OP Stack chain you want to sync (e.g. `reth`, `geth`, `besu`, `nethermind`, etc.)
1. A rollup node (e.g. `op-node`, `magi`, `hildr`, etc.)
1. An instance of `op-reth`.

For this example, we'll start a `Base Mainnet` node.

### Installing `op-reth`

To run Reth on Optimism, first install `op-reth` via the `Makefile` in the workspace root:

```sh
git clone git@github.com:paradigmxyz/reth.git && \
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
git clone git@github.com:ethereum-optimism/optimism.git && \
    (cd optimism/op-service/rethdb-reader && cargo build --release) && \ 
    cd optimism/op-node && \
    go build -v -tags rethdb -o ./bin/op-node ./cmd/main.go && \
    mv bin/op-node /usr/bin/op-node
```
This will build the `rethdb-reader` dylib and instruct the `op-node` build to statically link this dylib into the binary. The `op-node` binary will be installed to `/usr/bin/op-node`.

### Running `op-reth`

The `optimism` feature flag in `op-reth` adds several new CLI flags to the `reth` binary:
1. `--rollup.sequencer-http <uri>` - The sequencer endpoint to connect to. Transactions sent to the `op-reth` EL are also forwarded to this sequencer endpoint for inclusion, as the sequencer is the entity that builds blocks on OP Stack chains.
1. `--rollup.disable-tx-pool-gossip` - Disables gossiping of transactions in the mempool to peers. This can be ommitted for personal nodes, though providers should always opt to enable this flag.
1. `--rollup.enable-genesis-walkback` - Disables setting the forkchoice status to tip on startup, making the `op-node` walk back to genesis and verify the integrity of the chain before starting to sync. This can be ommitted unless a corruption of local chainstate is suspected.

Base's `rollup.json` files, which contain various configuration fields for the rollup, can be found in their [node][base-node] repository, under the respective L1 settlement layer's directory (`mainnet`, `goerli`, & `sepolia`).

First, ensure that your L1 archival node is running and synced to tip. Then, start `op-reth` with the `--rollup.sequencer-http` flag set to the `Base Mainnet` sequencer endpoint:
```sh
op-reth node \
    --chain base \
    --rollup.sequencer-http https://sequencer.base.org \
    --http \
    --ws \
    --authrpc.port 9551 \
    --authrpc.jwtsecret /path/to/jwt.hex
```

Then, once `op-reth` has been started, start up the `op-node`:
```sh
op-node \
    --l1=<your-L1-rpc> \
    --l2=http://localhost:9551 \
    --rollup.config=/path/to/rollup.json \
    --l2.jwt-secret=/path/to/jwt.hex \
    --rpc.addr=0.0.0.0 \
    --rpc.port=7000 \
    --l1.trustrpc
```

If you opted to build the `op-node` with the `rethdb` build tag, this "`RPCKind`" can be enabled via appending two extra flags to the `op-node` invocation:

> Note, the `reth_db_path` is the path to the `db` folder inside of the reth datadir, not the `mdbx.dat` file itself. This can be fetched from `op-reth db path [--chain <chain-name>]`, or if you are using a custom datadir location via the `--datadir` flag,
> by appending `/db` to the end of the path.

```sh
op-node \
    # ...
    --l1.rpckind=reth_db \
    --l1.rethdb=<your_L1_reth_db_path>
```

[l1-el-spec]: https://github.com/ethereum/execution-specs
[rollup-node-spec]: https://github.com/ethereum-optimism/optimism/blob/develop/specs/rollup-node.md
[op-geth-forkdiff]: https://op-geth.optimism.io
[sequencer]: https://github.com/ethereum-optimism/optimism/blob/develop/specs/introduction.md#sequencers
[op-stack-spec]: https://github.com/ethereum-optimism/optimism/tree/develop/specs
[l2-el-spec]: https://github.com/ethereum-optimism/optimism/blob/develop/specs/exec-engine.md
[deposit-spec]: https://github.com/ethereum-optimism/optimism/blob/develop/specs/deposits.md
[derivation-spec]: https://github.com/ethereum-optimism/optimism/blob/develop/specs/derivation.md

[op-node-docker]: https://console.cloud.google.com/artifacts/docker/oplabs-tools-artifacts/us/images/op-node

[reth]: https://github.com/paradigmxyz/reth
[op-node]: https://github.com/ethereum-optimism/optimism/tree/develop/op-node
[magi]: https://github.com/a16z/magi
[hildr]: https://github.com/optimism-java/hildr

[base-node]: https://github.com/base-org/node/tree/main
