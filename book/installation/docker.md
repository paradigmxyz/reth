# Docker

There are two ways to obtain a Reth Docker image:

1. [GitHub](#github)
2. [Building it from source](#building-the-docker-image)

Once you have obtained the Docker image, proceed to [Using the Docker
image](#using-the-docker-image).

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
    -v rethdata:/root/.local/share/reth/db \
    -d \
    -p 9000:9000 \
    --name reth \
    reth:local \
    node \
    --metrics 0.0.0.0:9000
```

The above command will create a container named `reth` and a named volume called `rethdata` for data persistence.

It will use the local image `reth:local`. If you want to use the GitHub Container Registry remote image, use `ghcr.io/paradigmxyz/reth` with your preferred tag.

### Using Docker Compose

To run Reth with Docker Compose, run the following command from a shell inside the root directory of this repository:

```bash
./etc/generate-jwt.sh
docker compose -f etc/docker-compose.yml up -d
```

To check if Reth is running correctly, run:

```bash
docker compose logs -f reth
```

The default `docker-compose.yml` file will create four containers:

- Reth
- Prometheus
- Grafana
- Lighthouse

Grafana will be exposed on `localhost:3000` and accessible via default credentials (username and password is `admin`)

## Interacting with Reth inside Docker

To interact with Reth you must first open a shell inside the Reth container by running:

```bash
docker exec -it reth bash
```

**If Reth is running with Docker Compose, replace `reth` with `reth-reth-1` in the above command**

### Listing the tables

```bash
reth db stats
```

### Viewing some records

```bash
reth db list --start=1 --len=2 Headers
```