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
1. [Plain Docker](#using-plain-docker)
2. [Using Docker compose](#using-docker-compose)

### Using Plain Docker

To run reth with Docker, run:

```bash
docker run \
    -v rethdata:$HOME/.local/share/reth/db \
    -v rethlogs:$HOME/.local/share/reth/db \
    -dp 9000:9000 \
    --name reth \
    reth:local \
    /reth/target/release/reth node \
    --metrics reth:9000 \
    --debug.tip ${RETH_TIP:-0x7d5a4369273c723454ac137f48a4f142b097aa2779464e6505f1b1c5e37b5382} \
    --log.directory $HOME
```

The above command will create a container named `reth` and two named volumes `rethdata` and `rethlogs` for data persistence. 

It will use the local image `reth:local`. If you want to use a remote image, use `ghcr.io/paradigmxyz/reth` with your preferred tag.

### Using Docker Compose

TODO

## Interacting with Reth inside Docker

TODO