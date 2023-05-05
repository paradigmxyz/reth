# Binaries

Precompiled binaries are available from the [GitHub releases page](https://github.com/paradigmxyz/reth/releases).
These are better managed by using [rethup](#using-rethup).

## Rethup

rethup is the Reth installer. It is a wrapper around the GitHub releases page, and allows you to install Reth from a specific branch, commit, or pull request from your terminal, easily.

Open your terminal and run the following command:

```sh
curl -L https://reth.paradigm.xyz | bash
```

This will install rethup, then simply follow the instructions on-screen,
which will make the `rethup` command available in your CLI.

Running `rethup` by itself will install the latest (nightly) precompiled binary for `reth`.
See `rethup --help` for more options, like installing from a specific version or commit.

> ℹ️ **Note**
>
> If you're on Windows, you will need to install and use [Git BASH](https://gitforwindows.org/) or [WSL](https://learn.microsoft.com/en-us/windows/wsl/install),
> as your terminal, since rethup currently does not support Powershell or Cmd.

You can use the different rethup flags to install reth from a specific branch, pull request, or path.

```sh
rethup --branch master
rethup --path path/to/reth
rethup --pr 1234
```

## From Github Releases

Alternatively, you can download the binaries from the [GitHub releases page](https://github.com/paradigmxyz/reth/releases).

Binaries are supplied for four platforms:

- `x86_64-unknown-linux-gnu`: AMD/Intel 64-bit processors (most desktops, laptops, servers)
- `x86_64-apple-darwin`: macOS with Intel chips
- `aarch64-unknown-linux-gnu`: 64-bit ARM processors (Raspberry Pi 4)
- `x86_64-windows`: Windows with 64-bit processors

Each binary is contained in a `.tar.gz` archive. For this example, lets assume the user needs
a `x86_64` binary:
1. Go to the [Releases](https://github.com/paradigmxyz/reth/releases) page and
   select the latest release.
1. Download the `reth-${VERSION}-x86_64-unknown-linux-gnu.tar.gz` binary. For example, to obtain the binary file for v0.0.1 (the latest version at the time of writing), a user can run the following commands in a linux terminal:
    ```bash
    cd ~
    curl -LO https://github.com/paradigmxyz/reth/releases/download/v0.0.1-alpha/reth-v0.0.1-alpha-x86_64-unknown-linux-gnu.tar.gz
    tar -xvf reth-v0.0.1-alpha-x86_64-unknown-linux-gnu.tar.gz
    ```
1. Test the binary with `./reth --version` (it should print the version).
1. (Optional) Move the `reth` binary to a location in your `PATH`, so the `reth` command can be called from anywhere. For example, to copy `reth` from the current directory to `usr/bin`, run `sudo cp reth /usr/bin`.