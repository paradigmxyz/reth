# Observability with Prometheus & Grafana

Reth exposes a number of metrics, which are listed [here][metrics]. We can serve them from an HTTP endpoint by adding the `--metrics` flag:

```bash
RUST_LOG=info reth node --metrics 127.0.0.1:9000
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

## Prometheus & Grafana

We're going to be using Prometheus to collect metrics off of the endpoint we set up, and use Grafana to scrape the metrics from Prometheus and define a dashboard with them.

Let's begin by installing both Prometheus and Grafana, which one can do with e.g. Homebrew:

<!-- TODO: Provide cross-platform guidance -->

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

This will start a Prometheus service which by default scrapes the metrics exposed at `localhost:9000`. If you launched reth with a different `--metrics` endpoint, you can change the Prometheus config file at `/usr/local/etc/prometheus/prometheus.yml` to point to the correct endpoint, and then restart the Prometheus service. You can also stop the service and launch Prometheus with a custom `prometheus.yml` like the one provided under [`etc/prometheus/prometheus.yml`](https://github.com/paradigmxyz/reth/blob/main/etc/prometheus/prometheus.yml) in this repo.

Next, open up "localhost:3000" in your browser, which is the default URL for Grafana. Here, "admin" is the default for both the username and password.

Once you've logged in, click on the gear icon in the lower left, and select "Data Sources". Click on "Add data source", and select "Prometheus" as the type. In the HTTP URL field, enter `http://localhost:9090`, this is the default endpoint for the Prometheus scrape endpoint. Finally, click "Save & Test".

As this might be a point of confusion, `localhost:9000`, which we supplied to `--metrics`, is the endpoint that Reth exposes, from which Prometheus collects metrics. Prometheus then exposes `localhost:9090` (by default) for other services (such as Grafana) to consume Prometheus metrics.

To configure the dashboard in Grafana, click on the squares icon in the upper left, and click on "New", then "Import". From there, click on "Upload JSON file", and select the example file in `reth/etc/grafana/overview.json`. Finally, select the Prometheus data source you just created, and click "Import".

And voilá, you should see your dashboard! If you're not yet connected to any peers, the dashboard will look like it's in an empty state, but once you are, you should see it start populating with data.

## Conclusion

In this runbook, we took you through starting the node, exposing different log levels, exporting metrics, and finally viewing those metrics in a Grafana dashboard.

This will all be very useful to you, whether you're simply running a home node and want to keep an eye on its performance, or if you're a contributor and want to see the effect that your (or others') changes have on Reth's operations.

[installation]: ./installation.md
[release-profile]: https://doc.rust-lang.org/cargo/reference/profiles.html#release
[docs]: https://github.com/paradigmxyz/reth/tree/main/docs
[metrics]: https://github.com/paradigmxyz/reth/blob/main/docs/design/metrics.md#current-metrics