# Docker

There are two ways to obtain a Reth Docker image:

1. [GitHub](#github)
2. [Building it from source](#building-the-docker-image)

Once you have obtained the Docker image, proceed to [Using the Docker
image](#using-the-docker-image).

> **Note**
>
> Reth requires Docker Engine version 20.10.10 or higher due to [missing support](https://docs.docker.com/engine/release-notes/20.10/#201010) for the `clone3` syscall in previous versions.
## GitHub

Reth docker images for both x86_64 and ARM64 machines are published with every release of reth on GitHub Container Registry.

You can obtain the latest image with:

```bash
docker pull ghcr.io/paradigmxyz/reth
```

Or a specific version (e.g. v0.0.1) with:

```bash
docker pull ghcr.io/paradigmxyz/reth:v0.0.1
```

You can test the image with:

```bash
docker run --rm ghcr.io/paradigmxyz/reth --version
```

If you can see the latest [Reth release](https://github.com/paradigmxyz/reth/releases) version, then you've successfully installed Reth via Docker.

## Building the Docker image

To build the image from source, navigate to the root of the repository and run:

```bash
docker build . -t reth:local
```

The build will likely take several minutes. Once it's built, test it with:

```bash
docker run reth:local --version
```

## Using the Docker image

There are two ways to use the Docker image:
1. [Using Docker](#using-plain-docker)
2. [Using Docker Compose](#using-docker-compose)

### Using Plain Docker

To run Reth with Docker, run:

```bash
docker run \
    -v rethdata:/root/.local/share/reth/mainnet/db \
    -d \
    -p 9001:9001 \
    -p 30303:30303 \
    -p 30303:30303/udp \
    --name reth \
    reth:local \
    node \
    --metrics 0.0.0.0:9001
```

The above command will create a container named `reth` and a named volume called `rethdata` for data persistence.
It will also expose the `30303` port (TCP and UDP) for peering with other nodes and the `9001` port for metrics.

It will use the local image `reth:local`. If you want to use the GitHub Container Registry remote image, use `ghcr.io/paradigmxyz/reth` with your preferred tag.

### Using Docker Compose

To run Reth with Docker Compose, run the following command from a shell inside the root directory of this repository:

```bash
./etc/generate-jwt.sh
docker compose -f etc/docker-compose.yml -f etc/lighthouse.yml up -d
```

> **Note**
>
> If you want to run Reth with a CL that is not Lighthouse:
>
> - The JWT for the consensus client can be found at `etc/jwttoken/jwt.hex` in this repository, after the `etc/generate-jwt.sh` script is run
> - The Reth Engine API is accessible on `localhost:8551`

To check if Reth is running correctly, run:

```bash
docker compose -f etc/docker-compose.yml -f etc/lighthouse.yml logs -f reth
```

The default `docker-compose.yml` file will create three containers:

- Reth
- Prometheus
- Grafana

The optional `lighthouse.yml` file will create two containers:

- Lighthouse
- [`ethereum-metrics-exporter`](https://github.com/ethpandaops/ethereum-metrics-exporter)

Grafana will be exposed on `localhost:3000` and accessible via default credentials (username and password is `admin`), with two available dashboards:
- reth
- Ethereum Metrics Exporter (works only if Lighthouse is also running)

## Interacting with Reth inside Docker

To interact with Reth you must first open a shell inside the Reth container by running:

```bash
docker exec -it reth bash
```

**If Reth is running with Docker Compose, replace `reth` with `reth-reth-1` in the above command**

Refer to the [CLI docs](../cli/cli.md) to interact with Reth once inside the Reth container.
