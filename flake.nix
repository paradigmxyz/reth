{
  inputs = {
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    utils.url = "github:numtide/flake-utils";
  };
  outputs = { self, nixpkgs, utils, ... }@inputs: utils.lib.eachDefaultSystem (system:
    let
      # toml info
      cargoTOML = (builtins.fromTOML (builtins.readFile ./Cargo.toml));
      packageVersion = cargoTOML.workspace.package.version;
      rustVersion = cargoTOML.workspace.package.rust-version;
      rustPkg = pkgs.rust-bin.stable."${rustVersion}".default.override {
        extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
      };
      # platform packages
      macPackages = pkgs.lib.optionals pkgs.stdenv.isDarwin (with pkgs.darwin.apple_sdk.frameworks; [ Security CoreFoundation CoreServices ]);
      linuxPackages = pkgs.lib.optionals pkgs.stdenv.isLinux (with pkgs; [
        libclang.lib
        llvmPackages.libcxxClang
      ]);
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
        hardeningDisable = [ "fortify" ];
        buildInputs = with pkgs; [
          macPackages
          linuxPackages
          rustPkg
          cargoDeps
        ];
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
      };

      defaultPackage = rustPlatform.buildRustPackage {
        pname = "reth";
        version = packageVersion;
        cargoLock = {
          lockFile = ./Cargo.lock;
        };
        checkFlags = [
          #this test breaks Read Only FS sandbox
          "--skip=cli::tests::parse_env_filter_directives"
        ];
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        nativeBuildInputs = (with pkgs;[
          # common packages
        ]) ++ macPackages ++ linuxPackages;
        src = ./.;
      };
    }
  );
}
