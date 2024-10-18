## Miscellaneous

This directory contains miscellaneous files, such as example Grafana dashboards and Prometheus configuration.

The files in this directory may undergo a lot of changes while reth is unstable, so do not expect them to necessarily be
up to date.

### Overview

- [**Prometheus**](./prometheus/prometheus.yml): An example Prometheus configuration.
- [**Grafana**](./grafana/): Example Grafana dashboards & data sources.

### Docker Compose

To run Reth, Grafana or Prometheus with Docker Compose, refer to
the [docker docs](/book/installation/docker.md#using-docker-compose).

Thank you for providing that context. I understand now that this text is already part of a subsection. I'll revise it to
fit better within that structure while still improving clarity and readability.

### Grafana

#### Adding a new metric to Grafana

To set up a new metric in the Reth Grafana dashboard:

1. Add the metric to the codebase following the [metrics section](../docs/design/metrics.md#creating-metrics)
   documentation.

2. Build the Reth image:
   ```bash
   docker build . -t reth:local
   ```
   Modify the [docker-compose](docker-compose.yml) file to use your locally built image for the Reth service.

3. Run Docker Compose:
   ```bash
   docker compose -f etc/docker-compose.yml -f etc/lighthouse.yml up -d
   ```

4. Access Grafana:
    - Open `http://localhost:3000/` in a browser
    - Log in with username and password `admin`
    - Navigate to the `Dashboards` tab

5. Create or modify a dashboard:
    - Select an existing dashboard or create a new one
    - Click `Add` > `Visualization` to create a new panel

6. Configure your metric panel:
    - Set a panel title and description
    - Select metric(s) from the `Metrics browser` or use the `PromQL` terminal
    - Document your metric(s) by setting units, legends,...
    - When adding multiple metrics, use field overwrites if needed

7. Save and arrange:
    - Click `Apply` to save the panel
    - Drag the panel to desired position on the dashboard

8. Export the dashboard:
    - Click `Share` > `Export`
    - Toggle `Export for sharing externally`
    - Click `Save to file`

9. Update dashboard file:
    - Replace the content of the corresponding file in the [dashboards folder](grafana/dashboards) with the exported
      JSON

Your new metric is now integrated into the Reth Grafana dashboard.

#### Import Grafana dashboards

Running Grafana in Docker makes it possible to import existing dashboards, refer
to [docs on how to run only Grafana in Docker](/book/installation/docker.md#using-docker-compose#run-only-grafana-in-docker).

