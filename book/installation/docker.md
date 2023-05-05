# Docker

There are two ways to obtain a Reth Docker image:

1. [Docker Hub](#docker-hub), or
2. By [building a Docker image from source](#building-the-docker-image).

Once you have obtained the docker image via one of these methods, proceed to [Using the Docker
image](#using-the-docker-image).

## Docker Hub

Reth maintains the [paradigmxyz/reth][docker_hub] Docker Hub repository which provides an easy
way to run Reth without building the image yourself.

Obtain the latest image with:

```bash
docker pull paradigmxyz/reth
```

Download and test the image with:

```bash
docker run paradigmxyz/reth reth --version
```

If you can see the latest [Reth release](https://github.com/paradigmxyz/reth/releases) version
(see example below), then you've successfully installed Reth via Docker.

## Building the Docker Image

To build the image from source, navigate to
the root of the repository and run:

```bash
docker build . -t reth:local
```

The build will likely take several minutes. Once it's built, test it with:

```bash
docker run reth:local reth --help
```

[docker_hub]: https://hub.docker.com/repository/docker/paradigmxyz/reth/

