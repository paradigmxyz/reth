{
  description = "A flake for building the reth project";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    crane.url = "github:ipetkov/crane";
    crane.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];

        pkgs = import nixpkgs {
          inherit system overlays;
          config.allowUnfree =
            true; # Needed for some potential dependencies or tools
        };

        # Select the rust toolchain
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "clippy" "rustfmt" ];
        };

        # Crane library for building Rust projects
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Common arguments for crane builds
        commonArgs = {
          #src = craneLib.cleanCargoSource (craneLib.path ./.);
          src = (craneLib.path ./.);
          # Add a pname here to silence the warning for the cargoArtifacts build
          pname = "reth-workspace-deps";
          # Features based on Makefile (excluding jemalloc for non-linux potentially,
          # but Nix builds usually target Linux where jemalloc is fine)
          cargoExtraArgs = "--features jemalloc,asm-keccak,min-debug-logs";

          buildInputs = with pkgs;
            [
              # Dependencies needed for linking, e.g., libmdbx might need these
              openssl
              pkg-config
            ] ++ lib.optionals pkgs.stdenv.isLinux [
              # Linux specific dependencies if any (jemalloc is handled by feature flag)
            ];

          nativeBuildInputs = with pkgs; [
            # Tools needed during the build process itself
            pkg-config
            cmake # Often needed for C dependencies like libmdbx
            clang # Sometimes preferred/needed by cc crate
            llvmPackages.libclang # For bindgen if used
            perl # <---- Added perl here for sha3-asm build script
          ];

          # Environment variables from Makefile's reproducible build (optional, Nix handles reproducibility)
          # SOURCE_DATE_EPOCH = builtins.toString (self.lastModified or 0);
          # CARGO_INCREMENTAL = "0";
          # LC_ALL = "C";
          # TZ = "UTC";
          # MDBX might need specific flags, especially for cross-compiling (handled by crane/cc crate often)
          # JEMALLOC_SYS_WITH_LG_PAGE = "16"; # Example if needed for aarch64 builds
        };

        # Build cached dependencies
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Build reth package
        reth = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "reth"; # This overrides the pname from commonArgs
        });

        # Build op-reth package
        op-reth = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "op-reth"; # This overrides the pname from commonArgs
          # Specify the binary and manifest path for op-reth
          cargoExtraArgs = commonArgs.cargoExtraArgs
            + " --bin op-reth --manifest-path crates/optimism/bin/Cargo.toml";
        });

        # Build MDBX tools (db-tools target)
        # This requires building C code from a subdirectory within a dependency.
        # It's often simpler to package these separately if needed, or add complex build steps.
        # Here's a basic attempt, might need refinement based on MDBX build system.
        mdbx-tools = pkgs.stdenv.mkDerivation {
          pname = "mdbx-tools";
          version = "0.1.0"; # Placeholder version
          src = ./crates/storage/libmdbx-rs/mdbx-sys/libmdbx;
          nativeBuildInputs = with pkgs; [ make gcc ];
          # Silence benchmark message as in Makefile
          makeFlags = [ "IOARENA=1" "tools" ];
          installPhase = ''
            mkdir -p $out/bin
            cp mdbx_chk $out/bin/
            cp mdbx_copy $out/bin/
            cp mdbx_dump $out/bin/
            cp mdbx_drop $out/bin/
            cp mdbx_load $out/bin/
            cp mdbx_stat $out/bin/
          '';
          # Ensure clean build environment
          preConfigure = ''
            make clean
          '';
        };

      in {

        packages = {
          inherit reth op-reth mdbx-tools;
          default = self.packages.${system}.reth;
        };

        apps = {
          reth = flake-utils.lib.mkApp { drv = self.packages.${system}.reth; };
          op-reth =
            flake-utils.lib.mkApp { drv = self.packages.${system}.op-reth; };
          default = self.apps.${system}.reth;
        };

        # Development shell providing tools from Makefile
        devShells.default = pkgs.mkShell {
          inputsFrom = [
            self.packages.${system}.reth
            self.packages.${system}.op-reth
            self.packages.${system}.mdbx-tools
          ];

          # Use the same nativeBuildInputs as the build itself
          nativeBuildInputs = commonArgs.nativeBuildInputs;

          buildInputs = commonArgs.buildInputs ++ (with pkgs; [
            # Rust toolchain components
            rustToolchain

            # Linting/Formatting tools from Makefile
            codespell
            dprint

            # Other tools potentially used
            gdb # For debugging

            # Tools for docker targets (if needed locally)
            # docker
            # docker-buildx

            # Tools for EF tests (might need specific libs/tools)
            wget
            tar

            # Tools for coverage
            llvm-tools # for llvm-cov
            cargo-nextest

            # Tools for cross-compilation (if desired in shell)
            # cross
          ]);

          # Environment variables for development
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          RUSTFLAGS =
            "-C link-arg=-lgcc"; # Example from Makefile cross-compile, adjust if needed

          # Add cargo-audit (optional security check)
          # buildInputs = buildInputs ++ [ pkgs.cargo-audit ];
          # shellHook = ''
          #   echo "Running cargo audit..."
          #   cargo audit --db ${advisory-db}
          # '';
          # Add pre-commit hook integration (optional)
          # buildInputs = buildInputs ++ [ pkgs.pre-commit ];
          # shellHook = ''
          #   pre-commit install -f --install-hooks
          #   echo "Pre-commit hooks installed."
          # '';
          shellHook = ''
            echo "Reth Nix development environment activated."
            echo "Provided packages: reth, op-reth"
            echo "Toolchain: $(rustc --version)"
            # Add other useful info or setup steps here
          '';
        };

        # Add a formatter check using dprint (matches lint-toml)
        formatter = pkgs.dprint;

        # Add checks (optional, can be slow)
        checks = {
          # Check formatting
          formatting =
            pkgs.runCommand "fmt-check" { buildInputs = [ pkgs.dprint ]; } ''
              ${pkgs.dprint}/bin/dprint check
              touch $out
            '';

          # Run clippy (matches clippy target)
          clippy = pkgs.runCommand "clippy-check" {
            nativeBuildInputs = [ rustToolchain ]
              ++ commonArgs.nativeBuildInputs;
            buildInputs = commonArgs.buildInputs;
            src = commonArgs.src;
          } ''
            export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library"
            cargo clippy --all-targets --all-features -- -D warnings
            touch $out
          '';

          # Run tests (matches test-unit, but uses cargo test)
          # Note: EF tests are complex and likely require network/external data,
          # often skipped in basic Nix checks.
          unit-tests = pkgs.runCommand "unit-tests" {
            nativeBuildInputs = with pkgs;
              [ rustToolchain cargo-nextest ] ++ commonArgs.nativeBuildInputs;
            buildInputs = commonArgs.buildInputs;
            src = commonArgs.src;
          } ''
            export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library"
            # Using cargo nextest as in Makefile
            cargo nextest run --locked --workspace --features 'jemalloc-prof' -E 'kind(lib)' -E 'kind(bin)' -E 'kind(proc-macro)'
            touch $out
          '';

          # Check for spelling errors (matches lint-codespell)
          codespell = pkgs.runCommand "codespell-check" {
            nativeBuildInputs = [ pkgs.codespell ];
            src = commonArgs.src;
          } ''
            ${pkgs.codespell}/bin/codespell --skip "*.json,./testing/ef-tests/ethereum-tests"
            touch $out
          '';
        };
      });
}
