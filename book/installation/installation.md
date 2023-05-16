# Installation

Reth runs on Linux and macOS (Windows tracked).

There are three core methods to obtain Reth:
* [Pre-built binaries](./binaries.md).
* [Docker images](./docker.md).
* [Building from source.](./source.md).

## Hardware Requirements

The hardware requirements for running Reth depend on the node configuration and can change over time as the network grows or new features are implemented. The most important requirement is by far the disk, whereas CPU and RAM requirements are relatively flexible.

### Disk

There are multiple types of disks to sync Reth, with varying size requirements, depending on the [syncing mode](../run/sync-modes.md):

* Archive Node: At least 2.5TB is required to store 
* Full Node: TBD

The time to sync also varies depending on the node type. NVMe drives are recommended for the best performance, however they can get expensive. SSDs are a good alternative, and HDDs are the cheapest option, but they will take the longest to sync.

### CPU

Most of the time in Ethereum is spent executing transactions, which is a single-threaded operation due to potential state dependencies of a transaction on previous ones. As a result, the number of cores matters less, but in general higher clock speeds are better. More cores are better for parallelizable [stages, like Senders Recovery, or Bodies](../developers/architecture.md), but these stages are not the bottleneck for syncing.

### Memory

It is recommended to use at least 8GB of RAM. 

Most of Reth's components tend to consume a low amount of memory, unless you are under heavy RPC load, so this should matter less than the other requirements.

Higher memory is generally better as it allows for better caching and less stress on the disk.

### Bandwidth

A stable and dependable internet connection is crucial for both syncing a node from genesis and for keeping up with the chain's tip. 

Note that due to Reth's staged sync, you only need an internet connection for the Headers and Bodies stages. This means that the first 1-3 hours (depending on your internet connection) will be online, downloading all necessary data, and the rest will be done offline and does not require an internet connection. 

Once you're synced to the tip you will need a reliable connection, especially if you're operating a validator. A 24MBps connection is recommended, but you can probably get away with less. Make sure your ISP does not cap your bandwidth.