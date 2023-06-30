## Miscellaneous

This directory contains miscellaneous files, such as example Grafana dashboards and Prometheus configuration.

The files in this directory may undergo a lot of changes while reth is unstable, so do not expect them to necessarily be up to date.

### Overview

- [**Prometheus**](./prometheus/prometheus.yml): An example Prometheus configuration.
- [**Grafana**](./grafana/): Example Grafana dashboards & data sources.

### Docker Compose

To run Reth + Lighthouse with Grafana dashboards and pre-configured Prometheus data sources pointing at
the locally running instances with metrics exposed on `localhost:9001` (reth) and `localhost:5054` (lighthouse):
```sh
docker compose -p reth -f ./etc/docker-compose.yml up
```

After that, Grafana will be exposed on `localhost:3000` and accessible without credentials.  Two dashboards
will be available:
- **Reth Overview**: A dashboard with a few key metrics from Reth.
- **Lighthouse Overview**: A dashboard with a few key metrics from Lighthouse.