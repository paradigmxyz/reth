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

### Grafana

#### Adding a new metric to Grafana

To set up a new metric in Reth and its Grafana dashboard (this assumes running Reth and Grafana instances):

1. Add the metric to the codebase following the [metrics section](../docs/design/metrics.md#creating-metrics)
   documentation.

1. Access Grafana:

    - Open `http://localhost:3000/` in a browser
    - Log in with username and password `admin`
    - Navigate to the `Dashboards` tab

1. Create or modify a dashboard:

    - Select an existing dashboard or create a new one
    - Click `Add` > `Visualization` to create a new panel

1. Configure your metric panel:

    - Set a panel title and description
    - Select metric(s) from the `Metrics browser` or use the `PromQL` terminal
    - Document your metric(s) by setting units, legends, etc.
    - When adding multiple metrics, use field overwrites if needed

1. Save and arrange:

    - Click `Apply` to save the panel
    - Drag the panel to desired position on the dashboard

1. Export the dashboard:

    - Click `Share` > `Export`
    - Toggle `Export for sharing externally`
    - Click `Save to file`

1. Update dashboard file:
    - Replace the content of the corresponding file in the [dashboards folder](./grafana/dashboards) with the exported
      JSON

Your new metric is now integrated into the Reth Grafana dashboard.

#### Import Grafana dashboards

If you are running Reth and Grafana outside of docker, and wish to import new Grafana dashboards or update a dashboard:

1. Go to `Home` > `Dashboards`

1. Click `New` > `Import`

1. Drag the JSON dashboard file to import it

1. If updating an existing dashboard, you will need to change the name and UID of the imported dashboard in order to
   avoid conflict

1. Delete the old dashboard

If you are running Reth and Grafana using docker, after having pulled the updated dashboards from `main`, restart the
Grafana service. This will update all dashboards.