# Running Reth

This chapter will provide a few runbooks for getting the node up and running.

This goal of this is not to expose all of the options available to you for running Reth (e.g. config file options, CLI flags, etc.) - those will be documented in the other chapters - but rather showcase some examples of different ways in which one could run the node.

## Run on MacOS w/ Prometheus + Grafana for Metrics

This will provide a brief overview for how to run Reth locally on a Mac.

First, ensure that you have Reth installed by following the [instructions to install on Mac][mac-installation].

### Basic operation

The most basic way to run it is by using the following command:

```bash
cargo run --release -- node
```

This will build Reth using cargo's [release profile][release-profile], and run it with an error log level by default.

You will likely see a large number of error logs to the tune of:

```bash
ERROR Disconnected by peer during handshake: Too many peers
```

These simply indicate that the peer you discovered is already connected to too many other peers and cannot accept a new connection with you. Nothing to worry about here, this is part of the peering process.

Believe it or not, your node is already running and attempting to sync.

When playing around with sync, you may want to set a threshold block that you'd like to sync up to instead of going for a full sync from the jump.

To do so, add the `--debug.tip` flag with the block hash that you'd like to sync to:

```bash
cargo run --release -- node --debug.tip 0x8e38b4dbf6b11fcc3b9dee84fb7986e29ca0a02cecd8977c161ff7333329681e
```

This will sync to block 1M.

That's great and all, but wouldn't it be nice to get a deeper view into what's going on?

### Adding more logs to the mix

Let's try again, now with the following command:

```bash
RUST_LOG=info,sync::stages=trace,downloaders=trace cargo run --release -- node --debug.tip 0x8e38b4dbf6b11fcc3b9dee84fb7986e29ca0a02cecd8977c161ff7333329681e
```

This will dump info-level logs throughout the node's operation, as well as trace-level logs for the pipeline stages and the downloaders (which fetch data over the P2P network). Check out the [docs][docs] for more info on these!

Now, you'll probably see a _lot_ of warning logs stemming from attempting to connect to peers, including but not limited to:
    - `error=Some(Eth(P2PStreamError(HandshakeError(NoResponse))))`
        - This means that the peer you're attempting to do a devp2p handshake with didn't send you any data
    - `error=Some(Eth(EthHandshakeError(MismatchedGenesis { expected: 0x..., got: 0x... })))`
        - This means you have a different genesis block from the peer, which is likely because they are a node in a different devp2p network (e.g. a testnet, or Polygon, etc.)
    - `error=Some(Eth(P2PStreamError(HandshakeError(NoSharedCapabilities))))`
        - This means the peer supports a devp2p subprotocol your node does not, e.g. they might be a light client (which Reth doesn't yet support).
    - `error=Some(Eth(P2PStreamError(Disconnected(ClientQuitting))))`
        - This means the peer you're trying to connect to is in the process of shutting down.

These warnings are also nothing to worry about, all of this is part of the normal peering process. But, this gives you a view into how difficult finding good peers is!

You may want to keep these logs around outside of your terminal. To accomplish this, let's run:

```bash
RUST_LOG=info,sync::stages=trace,downloaders=trace cargo r --release -- node --debug.tip 0x8e38b4dbf6b11fcc3b9dee84fb7986e29ca0a02cecd8977c161ff7333329681e --log.directory ./
```

Here, adding `--log.directory` specifies a location to which the logs will be saved (in a file named `reth.log`) so that you can view them with a tool like `more`, `less`, or `tail`.

Now, trying to get a sense of sync progress by scanning through the logs is quite painful. Let's start consuming some metrics.

### Exporting metrics

Reth exposes a number of metrics, which are listed [here][metrics]. We can serve them from an HTTP endpoint by adding the `--metrics` flag:

```bash
RUST_LOG=info,sync::stages=trace,downloaders=trace nohup cargo r --release -- node --debug.tip 0x8e38b4dbf6b11fcc3b9dee84fb7986e29ca0a02cecd8977c161ff7333329681e --metrics '127.0.0.1:9000' > reth-out.txt &
```

Now, as the node is running, you can `curl` the endpoint you provided to the `--metrics` flag to get a text dump of the metrics at that time:

```bash
curl 127.0.0.1:9000
```

The response from this is quite descriptive, but it can be a bit verbose. Plus, it's just a snapshot of the metrics at the time that you `curl`ed the endpoint.

You can run the following command in a separate terminal to periodically poll the endpoint, and just print the values (without the header text) to the terminal:

```bash
while true; do date; curl -s localhost:9000 | grep -Ev '^(#|$)' | sort; echo; sleep 10; done
```

We're finally getting somewhere! As a final step, though, wouldn't it be great to see how these metrics progress over time (and generally, in a GUI)?

### Consuming metrics with Prometheus & Grafana

We're going to be using Prometheus to collect metrics off of the endpoint we set up, and use Grafana to scrape the metrics from Prometheus and define a dashboard with them.

Let's begin by installing both Prometheus and Grafana, which one can do with e.g. Homebrew:

```bash
brew update
brew install prometheus
brew install grafana
```

Then, kick off the Prometheus and Grafana services:

```bash
brew services start prometheus
brew services start grafana
```

After this, let's run Prometheus to start collecting metrics. In the root of the `reth` repo, run:

```bash
prometheus --config.file etc/prometheus/prometheus.yml
```

This uses the example Prometheus configuration file in the repo. Most importantly, be sure that the URL in the `targets` field of this config file matches the endpoint that you supplied to the `--metrics` flag.

Next, open up "localhost:3000" in your browser, which is the default URL for Grafana. Here, "admin" is the default for both the username and password.

Once you've logged in, click on the gear icon in the lower left, and select "Data Sources". Click on "Add data source", and select "Prometheus" as the type. In the HTTP URL field, enter "http://localhost:9090", this is the default endpoint for the Prometheus scrape endpoint. Finally, click "Save & Test".

As this might be a point of confusion, "localhost:9000", which we supplied to `--metrics`, is the endpoint that Reth exposes, from which Prometheus collects metrics. Prometheus then exposes "localhost:9090" (by default) for other services (such as Grafana) to consume Prometheus metrics.

To configure the dashboard in Grafana, click on the squares icon in the upper left, and click on "New", then "Import". From there, click on "Upload JSON file", and select the example file in `reth/etc/grafana/overview.json`. Finally, select the Prometheus data source you just created, and click "Import".

And voilá, you should see your dashboard! If you're not yet connected to any peers, the dashboard will look like it's in an empty state, but once you are, you should see it start populating with data.

### Conclusion

In this runbook, we took you through starting the node, exposing different log levels, exporting metrics, and finally viewing those metrics in a Grafana dashboard.

This will all be very useful to you, whether you're simply running a home node and want to keep an eye on its performance, or if you're a contributor and want to see the effect that your (or others') changes have on Reth's operations.

[mac-installation]: ./installation.md#macos
[release-profile]: https://doc.rust-lang.org/cargo/reference/profiles.html#release
[docs]: https://github.com/paradigmxyz/reth/tree/main/docs
[metrics]: https://github.com/paradigmxyz/reth/blob/main/docs/design/metrics.md#current-metrics
