## Miscellaneous

This directory contains miscellaneous files, such as example Grafana dashboards and Prometheus configuration.

The files in this directory may undergo a lot of changes while reth is unstable, so do not expect them to necessarily be up to date.

### Overview

- [**Prometheus**](./prometheus/prometheus.yml): An example Prometheus configuration.
- [**Grafana**](./grafana/): Example Grafana dashboards & data sources.

### Docker Compose

To run Grafana dashboard with example dashboard and pre-configured Prometheus data source pointing at
the locally running Reth instance with metrics exposed on `localhost:9001`:
```sh
docker compose -p reth -f ./etc/docker-monitoring.yml up
```

After that, Grafana will be exposed on `localhost:3000` and accessible via default credentials:
```
username: admin
password: admin
```