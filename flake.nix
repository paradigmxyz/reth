{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/25.11";
    nixpkgs-llvm22.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      nixpkgs-llvm22,
      utils,
      crane,
      fenix,
      ...
    }:
    utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        llvm22Pkgs = import nixpkgs-llvm22 { inherit system; };

        # A useful helper for folding a list of `prevSet -> newSet` functions
        # into an attribute set.
        composeAttrOverrides = defaultAttrs: overrides: builtins.foldl'
            (acc: f: acc // (f acc)) defaultAttrs overrides;

        cargoTarget = pkgs.stdenv.hostPlatform.rust.rustcTargetSpec;
        cargoTargetEnvVar = builtins.replaceStrings ["-"] ["_"]
            (pkgs.lib.toUpper cargoTarget);

        cargoTOML = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        packageVersion = cargoTOML.workspace.package.version;

        rustStable = fenix.packages.${system}.stable.withComponents [
          "cargo" "rustc" "rust-src" "clippy"
        ];

        rustNightly = fenix.packages.${system}.latest;

        craneLib = (crane.mkLib pkgs).overrideToolchain rustStable;

        llvm22 = llvm22Pkgs.llvmPackages_22;

        nativeBuildInputs = [
          pkgs.pkg-config
          pkgs.libgit2
          pkgs.m4
          pkgs.perl
        ];

        withClang = prev: {
          buildInputs = prev.buildInputs or [] ++ [
            pkgs.clang
          ];
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
        };

        withLlvm22 = prev: {
          nativeBuildInputs = prev.nativeBuildInputs or [] ++ [
            llvm22.llvm.dev
          ];
          buildInputs = prev.buildInputs or [] ++ [
            llvm22.llvm.lib
          ];
          LLVM_SYS_221_PREFIX = "${llvm22.llvm.dev}";
        };

        withLlvm22DevShell = prev: (withLlvm22 prev) // {
          packages = prev.packages or [] ++ [
            llvm22.llvm.dev
            llvm22.llvm.lib
          ];
        };

        withMaxPerf = prev: {
          cargoBuildCommand = "cargo build --profile=maxperf";
          RUSTFLAGS = prev.RUSTFLAGS or [] ++ [
            "-Ctarget-cpu=native"
          ];
        };

        withMold = prev: {
          buildInputs = prev.buildInputs or [] ++ [
            pkgs.mold
          ];
          "CARGO_TARGET_${cargoTargetEnvVar}_LINKER" = "${pkgs.llvmPackages.clangUseLLVM}/bin/clang";
          RUSTFLAGS = prev.RUSTFLAGS or [] ++ [
            "-Clink-arg=-fuse-ld=${pkgs.mold}/bin/mold"
          ];
        };

        mkReth = overrides: craneLib.buildPackage (composeAttrOverrides {
          pname = "reth";
          version = packageVersion;
          src = ./.;
          inherit nativeBuildInputs;
          doCheck = false;
        } overrides);

      in
      {
        packages = rec {

          reth = mkReth ([
            withClang
            withLlvm22
            withMaxPerf
          ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            withMold
          ]);

          default = reth;
        };

        devShell = let
          overrides = [
            withClang
            withLlvm22DevShell
          ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            withMold
          ];
        in craneLib.devShell (composeAttrOverrides {
          packages = nativeBuildInputs ++ [
            rustNightly.rust-analyzer
            rustNightly.rustfmt
            pkgs.cargo-nextest
          ];

          # Remove the hardening added by nix to fix jmalloc compilation error.
          # More info: https://github.com/tikv/jemallocator/issues/108
          hardeningDisable = [ "fortify" ];

        } overrides);
      }
    );
}
