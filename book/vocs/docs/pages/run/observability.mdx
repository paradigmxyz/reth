# Observability with Prometheus & Grafana

Reth exposes a number of metrics which can be enabled by adding the `--metrics` flag:

```bash
reth node --metrics 127.0.0.1:9001
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

We're going to use Prometheus to scrape the metrics from our node, and use Grafana to on a dashboard.

Let's begin by installing both Prometheus and Grafana:

### macOS

Using Homebrew:

```bash
brew update
brew install prometheus
brew install grafana
```

### Linux

#### Debian/Ubuntu
```bash
# Install Prometheus
# Visit https://prometheus.io/download/ for the latest version
PROM_VERSION=$(curl -s https://api.github.com/repos/prometheus/prometheus/releases/latest | grep tag_name | cut -d '"' -f 4 | cut -c 2-)
wget https://github.com/prometheus/prometheus/releases/download/v${PROM_VERSION}/prometheus-${PROM_VERSION}.linux-amd64.tar.gz
tar xvfz prometheus-*.tar.gz
cd prometheus-*

# Install Grafana
sudo apt-get install -y apt-transport-https software-properties-common
wget -q -O - https://packages.grafana.com/gpg.key | sudo apt-key add -
echo "deb https://packages.grafana.com/oss/deb stable main" | sudo tee -a /etc/apt/sources.list.d/grafana.list
sudo apt-get update
sudo apt-get install grafana
```

#### Fedora/RHEL/CentOS
```bash
# Install Prometheus
# Visit https://prometheus.io/download/ for the latest version
PROM_VERSION=$(curl -s https://api.github.com/repos/prometheus/prometheus/releases/latest | grep tag_name | cut -d '"' -f 4 | cut -c 2-)
wget https://github.com/prometheus/prometheus/releases/download/v${PROM_VERSION}/prometheus-${PROM_VERSION}.linux-amd64.tar.gz
tar xvfz prometheus-*.tar.gz
cd prometheus-*

# Install Grafana
# Visit https://grafana.com/grafana/download for the latest version
sudo dnf install -y https://dl.grafana.com/oss/release/grafana-latest-1.x86_64.rpm
```

### Windows

#### Using Chocolatey
```powershell
choco install prometheus
choco install grafana
```

#### Manual installation
1. Download the latest Prometheus from [prometheus.io/download](https://prometheus.io/download/)
   - Select the Windows binary (.zip) for your architecture (typically windows-amd64)
2. Download the latest Grafana from [grafana.com/grafana/download](https://grafana.com/grafana/download)
   - Choose the Windows installer (.msi) or standalone version
3. Extract Prometheus to a location of your choice (e.g., `C:\prometheus`)
4. Install Grafana by running the installer or extracting the standalone version
5. Configure Prometheus and Grafana to run as services if needed

Then, kick off the Prometheus and Grafana services:

```bash
# For macOS
brew services start prometheus
brew services start grafana

# For Linux (systemd-based distributions)
sudo systemctl start prometheus
sudo systemctl start grafana-server

# For Windows (if installed as services)
Start-Service prometheus
Start-Service grafana
```

This will start a Prometheus service which [by default scrapes itself about the current instance](https://prometheus.io/docs/introduction/first_steps/#:~:text=The%20job%20contains%20a%20single,%3A%2F%2Flocalhost%3A9090%2Fmetrics.). So you'll need to change its config to hit your Reth node’s metrics endpoint at `localhost:9001` which you set using the `--metrics` flag.

You can find an example config for the Prometheus service in the repo here: [`etc/prometheus/prometheus.yml`](https://github.com/paradigmxyz/reth/blob/main/etc/prometheus/prometheus.yml)

Depending on your installation you may find the config for your Prometheus service at:

- OSX: `/opt/homebrew/etc/prometheus.yml`
- Linuxbrew: `/home/linuxbrew/.linuxbrew/etc/prometheus.yml`
- Others: `/usr/local/etc/prometheus/prometheus.yml`

Next, open up "localhost:3000" in your browser, which is the default URL for Grafana. Here, "admin" is the default for both the username and password.

Once you've logged in, click on "Connections" in the left side panel and select "Data Sources". Click on "Add data source", and select "Prometheus" as the type. In the HTTP URL field, enter http://localhost:9090. Finally, click "Save & Test".

As this might be a point of confusion, `localhost:9001`, which we supplied to `--metrics`, is the endpoint that Reth exposes, from which Prometheus collects metrics. Prometheus then exposes `localhost:9090` (by default) for other services (such as Grafana) to consume Prometheus metrics.

To configure the dashboard in Grafana, click on the squares icon in the upper left, and click on "New", then "Import". From there, click on "Upload JSON file", and select the example file in [`reth/etc/grafana/dashboards/overview.json`](https://github.com/paradigmxyz/reth/blob/main/etc/grafana/dashboards/overview.json). Finally, select the Prometheus data source you just created, and click "Import".

And voilà, you should see your dashboard! If you're not yet connected to any peers, the dashboard will look like it's in an empty state, but once you are, you should see it start populating with data.

## Conclusion

In this runbook, we took you through starting the node, exposing different log levels, exporting metrics, and finally viewing those metrics in a Grafana dashboard.

This will all be very useful to you, whether you're simply running a home node and want to keep an eye on its performance, or if you're a contributor and want to see the effect that your (or others') changes have on Reth's operations.

[installation]: ../installation/installation.md
[release-profile]: https://doc.rust-lang.org/cargo/reference/profiles.html#release
[docs]: https://github.com/paradigmxyz/reth/tree/main/docs
[metrics]: https://github.com/paradigmxyz/reth/blob/main/docs/design/metrics.md#current-metrics
