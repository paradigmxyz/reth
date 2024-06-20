# Remote Execution Extensions

In this chapter, we will learn how to create an ExEx that emits all notifications to an external process.

We will use [Tonic](https://github.com/hyperium/tonic) to create a gRPC server and a client.
- The server binary will have the Reth client, our ExEx and the gRPC server.
- The client binary will have the gRPC client that connects to the server.

### Prerequisites

See [section](https://github.com/hyperium/tonic?tab=readme-ov-file#dependencies) of the Tonic documentation
to install the required dependencies.

### Create a new project


