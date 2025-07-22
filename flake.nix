{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    utils.url = "github:numtide/flake-utils";
  };
  outputs =
    {
      nixpkgs,
      utils,
      ...
    }@inputs:
    utils.lib.eachDefaultSystem (
      system:
      let
        # Rust
        cargoTOML = (builtins.fromTOML (builtins.readFile ./Cargo.toml));
        packageVersion = cargoTOML.workspace.package.version;
        rustVersion = cargoTOML.workspace.package.rust-version;
        rustPkg = pkgs.rust-bin.stable."${rustVersion}".default;

        # Packages
        linuxPackages = pkgs.lib.optionals pkgs.stdenv.isLinux (
          with pkgs;
          [
            libclang.lib
            llvmPackages.libcxxClang
          ]
        );
        overlays = [ (import inputs.rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        cargoDeps = pkgs.rustPlatform.importCargoLock {
          lockFile = ./Cargo.lock;
        };

        rustPlatform = pkgs.makeRustPlatform {
          rustc = rustPkg;
          cargo = rustPkg;
        };
      in
      {
        devShell = pkgs.mkShell {
          buildInputs = with pkgs; [
            linuxPackages
            rustup
            cargoDeps
          ];

          # We want to be able to use `cargo +nightly`, so we need to install nightly toolchain via `rustup`
          shellHook = ''
            ${pkgs.lib.getExe pkgs.rustup} -q toolchain install ${rustVersion} nightly > /dev/null
            ${pkgs.lib.getExe pkgs.rustup} -q default ${rustVersion} > /dev/null
            ${pkgs.lib.getExe pkgs.rustup} -q component add --toolchain nightly rust-analyzer clippy rustfmt > /dev/null
          '';
        };

        defaultPackage = rustPlatform.buildRustPackage {
          pname = "reth";
          version = packageVersion;
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
          RUSTFLAGS = "-C target-cpu=native";
          buildType = "maxperf";
          buildFeatures = [
            "jemalloc"
            "asm-keccak"
          ];
          doCheck = false;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          nativeBuildInputs =
            (with pkgs; [
              perl # Required for asm-keccak feature
            ])
            ++ linuxPackages;
          src = ./.;
        };
      }
    );
}
