# Review of other codebases

This document contains some of our research in how other codebases designed various parts of their stack.

## P2P

* [`Sentry`](https://github.com/vorot93/sentry), a pluggable p2p node following the [Erigon gRPC architecture](https://erigon.substack.com/p/current-status-of-silkworm-and-silkrpc):
    * [`vorot93`](https://github.com/vorot93/) first started by implementing a rust devp2p stack in [`devp2p`](https://github.com/vorot93/devp2p)
    * vorot93 then started work on sentry, using devp2p, to satisfy the erigon architecture of modular components connected with gRPC.
    * The code from rust-ethereum/devp2p was merged into sentry, and rust-ethereum/devp2p was archived
    * vorot93 was working concurrently on akula, which used sentry to connect to the network.
    * The code from sentry was merged into akula, and sentry was deleted (hence the 404 on the sentry link)

* [`Ranger`](https://github.com/Rjected/ranger), an ethereum p2p client capable of interacting with peers without a full node:
    * [Rjected](https://github.com/Rjected/) built Ranger for p2p network research
    * It required extracting devp2p-rs from Akula's sentry directory to a separate repository (keeping the GPL License), so that it could be used in Ranger (also GPL Licensed)
    * It also required creating [`ethp2p`](https://github.com/Rjected/ethp2p), a clean room implementation of the [eth wire](https://github.com/ethereum/devp2p/blob/master/caps/eth.md) protocol, for use in ranger.

## Database

* [Erigon's DB walkthrough](https://github.com/ledgerwatch/erigon/blob/12ee33a492f5d240458822d052820d9998653a63/docs/programmers_guide/db_walkthrough.MD) contains an overview. They made the most noticeable improvements on storage reduction.
* [Erigon Videos](https://youtu.be/QqL72qWhF-g) explain new proposals in improving in future versions and take some insights from it. (example: CumulativeTxCount, EliasFano)
* [Gio's erigon-db table macros](https://github.com/gio256/erigon-db) + [Akula's macros](https://github.com/akula-bft/akula/blob/74b172ee1d2d2a4f04ce057b5a76679c1b83df9c/src/kv/tables.rs#L61).

## Header Downloaders

* Erigon Header Downloader:
    * A header downloader algo was introduced in [`erigon#1016`](https://github.com/ledgerwatch/erigon/pull/1016) and finished in [`erigon#1145`](https://github.com/ledgerwatch/erigon/pull/1145). At a high level, the downloader concurrently requested headers by hash, then sorted, validated and fused the responses into chain segments. Smaller segments were fused into larger as the gaps between them were filled. The downloader also used to maintain hardcoded hashes (later renamed to preverified) to bootstrap the sync.
    * The downloader was refactored multiple times: [`erigon#1471`](https://github.com/ledgerwatch/erigon/pull/1471), [`erigon#1559`](https://github.com/ledgerwatch/erigon/pull/1559) and [`erigon#2035`](https://github.com/ledgerwatch/erigon/pull/2035).
    * With PoS transition in [`erigon#3075`](https://github.com/ledgerwatch/erigon/pull/3075) terminal td was introduced to the algo to stop forward syncing. For the downward sync (post merge), the download was now delegated to [`EthBackendServer`](https://github.com/ledgerwatch/erigon/blob/3c95db00788dc740849c2207d886fe4db5a8c473/ethdb/privateapi/ethbackend.go#L245)
    * Proper reverse PoS downloader was introduced in [`erigon#3092`](https://github.com/ledgerwatch/erigon/pull/3092) which downloads the header batches from tip until local head is reached. Refactored later in [`erigon#3340`](https://github.com/ledgerwatch/erigon/pull/3340) and [`erigon#3717`](https://github.com/ledgerwatch/erigon/pull/3717).

* Akula Headers & Stage Downloader:
    * What seems to be the first working version was wired together in [`akula#89`](https://github.com/akula-bft/akula/pull/89). The headers stage invoked the downloader which created a staged stream for [header download](https://github.com/akula-bft/akula/blob/7dfdca134557993fe47fa54750616d3d167187c7/src/downloader/headers/downloader_linear.rs#L135-L149). The rough description of the process would be request -> receive response -> retry if necessary -> verify response -> verify attachment -> save -> refill (flush?). Same as erigon, it used to rely on preverified hashes to bootstrap the download process.
    * A Concurrent downloader was introduced in [`akula#bbde8d`](https://github.com/akula-bft/akula/commit/bbde8d778184c87621ef9ffdbb0cb15f0e17964f). It dispatched multiple requests, collected & verified responses and afterwards inserted the headers. The same logic was refactored in [`akula#38381e`](https://github.com/akula-bft/akula/commit/38381e0b1de752a46216bf1cb0afad5547b87733).
    * Watching chain tip changes was introduced in [`akula#cdc083`](https://github.com/akula-bft/akula/commit/cdc083ff24c0666e29257a714fd2899ed699bee6).
    * Proper consensus engine together with reverse download was introduced in [`akula#fcc1a08`](https://github.com/akula-bft/akula/commit/fcc1a08e4a7ec4955360276d6c8b381ddb82af42). The majority of code from this commit is akula's headers stage as we know it today.
