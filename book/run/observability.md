# Observability with Prometheus & Grafana

Reth exposes a number of metrics, which are listed [here][metrics]. We can serve them from an HTTP endpoint by adding the `--metrics` flag:

```bash
RUST_LOG=info reth node --metrics 127.0.0.1:9001
```

Now, as the node is running, you can `curl` the endpoint you provided to the `--metrics` flag to get a text dump of the metrics at that time:

```bash
curl 127.0.0.1:9001
```

The response from this is quite descriptive, but it can be a bit verbose. Plus, it's just a static_file of the metrics at the time that you `curl`ed the endpoint.

You can run the following command in a separate terminal to periodically poll the endpoint, and just print the values (without the header text) to the terminal:

```bash
while true; do date; curl -s localhost:9001 | grep -Ev '^(#|$)' | sort; echo; sleep 10; done
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

This will start a Prometheus service which [by default scrapes itself about the current instance](https://prometheus.io/docs/introduction/first_steps/#:~:text=The%20job%20contains%20a%20single,%3A%2F%2Flocalhost%3A9090%2Fmetrics.). So you'll need to change its config to hit your Reth nodes metrics endpoint at `localhost:9001` which you set using the `--metrics` flag.

You can find an example config for the Prometheus service in the repo here: [`etc/prometheus/prometheus.yml`](https://github.com/paradigmxyz/reth/blob/main/etc/prometheus/prometheus.yml)

Depending on your installation you may find the config for your Prometheus service at:

- OSX: `/opt/homebrew/etc/prometheus.yml`
- Linuxbrew: `/home/linuxbrew/.linuxbrew/etc/prometheus.yml`
- Others: `/usr/local/etc/prometheus/prometheus.yml`

Next, open up "localhost:3000" in your browser, which is the default URL for Grafana. Here, "admin" is the default for both the username and password.

Once you've logged in, click on the gear icon in the lower left, and select "Data Sources". Click on "Add data source", and select "Prometheus" as the type. In the HTTP URL field, enter `http://localhost:9090`. Finally, click "Save & Test".

As this might be a point of confusion, `localhost:9001`, which we supplied to `--metrics`, is the endpoint that Reth exposes, from which Prometheus collects metrics. Prometheus then exposes `localhost:9090` (by default) for other services (such as Grafana) to consume Prometheus metrics.

To configure the dashboard in Grafana, click on the squares icon in the upper left, and click on "New", then "Import". From there, click on "Upload JSON file", and select the example file in [`reth/etc/grafana/dashboards/overview.json`](https://github.com/paradigmxyz/reth/blob/main/etc/grafana/dashboards/overview.json). Finally, select the Prometheus data source you just created, and click "Import".

And voilá, you should see your dashboard! If you're not yet connected to any peers, the dashboard will look like it's in an empty state, but once you are, you should see it start populating with data.

## Conclusion

In this runbook, we took you through starting the node, exposing different log levels, exporting metrics, and finally viewing those metrics in a Grafana dashboard.

This will all be very useful to you, whether you're simply running a home node and want to keep an eye on its performance, or if you're a contributor and want to see the effect that your (or others') changes have on Reth's operations.

[installation]: ../installation/installation.md
[release-profile]: https://doc.rust-lang.org/cargo/reference/profiles.html#release
[docs]: https://github.com/paradigmxyz/reth/tree/main/docs
[metrics]: https://github.com/paradigmxyz/reth/blob/main/docs/design/metrics.md#current-metrics
