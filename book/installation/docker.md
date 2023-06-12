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
docker run --rm ghcr.io/paradigmxyz/reth reth --version
```

If you can see the latest [Reth release](https://github.com/paradigmxyz/reth/releases) version, then you've successfully installed Reth via Docker.

## Building the Docker image

To build the image from source, navigate to the root of the repository and run:

```bash
docker build . -t reth:local
```

The build will likely take several minutes. Once it's built, test it with:

```bash
docker run reth:local reth --version
```
