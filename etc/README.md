## Miscellaneous

This directory contains miscellaneous files, such as example Grafana dashboards and Prometheus configuration.

The files in this directory may undergo a lot of changes while reth is unstable, so do not expect them to necessarily be up to date.

### Overview

- [**Prometheus**](./prometheus/prometheus.yml): An example Prometheus configuration.
- [**Grafana**](./grafana/): Example Grafana dashboards & data sources.

### Docker Compose

To run Reth, Grafana or Prometheus with Docker Compose, refer to the [docker docs](/book/installation/docker.md#using-docker-compose).

### Import Grafana dashboards

Running Grafana in Docker makes it possible to import existing dashboards, refer to [docs on how to run only Grafana in Docker](/book/installation/docker.md#using-docker-compose#run-only-grafana-in-docker).