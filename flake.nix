{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      utils,
      rust-overlay,
      fenix,
      ...
    }:
    utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [
          (import rust-overlay)
          fenix.overlays.default
        ];
        pkgs = import nixpkgs { inherit system overlays; };

        cargoTOML = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        packageVersion = cargoTOML.workspace.package.version;
        rustVersion = cargoTOML.workspace.package."rust-version";

        rustPkg = pkgs.rust-bin.stable."${rustVersion}".default;
        nightly = pkgs.fenix.latest;

        rustPlatform = pkgs.makeRustPlatform {
          rustc = rustPkg;
          cargo = rustPkg;
        };

        linuxNative = pkgs.lib.optionals pkgs.stdenv.isLinux (
          with pkgs;
          [
            llvmPackages.libclang
            llvmPackages.libcxxClang
          ]
        );

        commonArgs = name: {
          pname = name;
          version = packageVersion;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.perl ] ++ linuxNative;
          RUSTFLAGS = "-C target-cpu=native";
          buildType = "maxperf";
          buildFeatures = [
            "jemalloc"
            "asm-keccak"
          ];
          doCheck = false;
        };
      in
      {
        packages = rec {
          reth = rustPlatform.buildRustPackage (commonArgs "reth");

          op-reth = rustPlatform.buildRustPackage (
            commonArgs "op-reth"
            // {
              buildAndTestSubdir = "crates/optimism/bin";
              cargoBuildFlags = [
                "--bin"
                "op-reth"
              ];
            }
          );

          default = reth;
        };

        devShell = pkgs.mkShell {
          buildInputs = [
            rustPkg
            nightly.rust-analyzer
            nightly.clippy
            nightly.rustfmt
          ]
          ++ linuxNative;
        };
      }
    );
}
