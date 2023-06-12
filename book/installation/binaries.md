# Binaries

Precompiled binaries are available from the [GitHub releases page](https://github.com/paradigmxyz/reth/releases).

Binaries are supplied for 5 platforms:

- Linux x86_64: AMD and Intel 64-bit processors (most desktops, laptops, and servers)
- Linux ARM64: 64-bit arm processors
- macOS x86_64: macOS with Intel chips
- macOS ARM64: macOS with Apple Silicon
- Windows x86_64: AMD and Intel 64-bit processors

Each binary is contained in a tarball.

As an example, you could install the Linux x86_64 version like so:

1. Go to the [Releases](https://github.com/paradigmxyz/reth/releases) page and select the latest release.
1. Download the `reth-${VERSION}-x86_64-unknown-linux-gnu.tar.gz` tarball.  

   For example, to obtain the binary file for v0.0.1-alpha, you can run the following commands in a Linux terminal:
   ```bash
   cd ~
   curl -LO https://github.com/paradigmxyz/reth/releases/download/v0.0.1-alpha/reth-v0.0.1-alpha-x86_64-unknown-linux-gnu.tar.gz
   tar -xvf reth-v0.0.1-alpha-x86_64-unknown-linux-gnu.tar.gz
   ```
1. Test the binary with `./reth --version` (it should print the version).
2. (Optional) Move the `reth` binary to a location in your `PATH`, so the `reth` command can be called from anywhere.  
   For most Linux distros, you can move the binary to `/usr/local/bin`: `sudo cp ./reth /usr/local/bin`.