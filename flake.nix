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
      macPackages = pkgs.lib.optionals pkgs.stdenv.isDarwin (with pkgs.darwin.apple_sdk.frameworks; [ Security CoreFoundation CoreServices ]);
      linuxPackages = pkgs.lib.optionals pkgs.stdenv.isLinux (with pkgs; [
        libclang.lib
        llvmPackages.libcxxClang
        clang
        openssl
      ]);
      overlays = [ (import inputs.rust-overlay) ];
      pkgs = import nixpkgs {
        inherit system overlays;
      };
      cargoDeps = pkgs.rustPlatform.importCargoLock {
        lockFile = ./Cargo.lock;
      };
      rust = pkgs.makeRustPlatform {
        inherit (inputs.fenix.packages.${system}.minimal) cargo rustc;
      };
      rustPlatform = pkgs.makeRustPlatform {
        rustc = pkgs.rust-bin.stable."1.81.0".default;
        cargo = pkgs.rust-bin.stable."1.81.0".default;
      };
    in
    {
      devShell = pkgs.mkShell {
        buildInputs = with pkgs; [
          macPackages
          linuxPackages
          cargoDeps
          (rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
          })
        ];
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
      };
      #defaultPackage = pkgs.rustPlatform.buildRustPackage {

      defaultPackage = rustPlatform.buildRustPackage {
        pname = "reth";
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
        cargoLock = {
          lockFile = ./Cargo.lock;
        };
        checkFlags = [
          #this test breaks Read Only FS sandbox
          "--skip=cli::tests::parse_env_filter_directives"
        ];
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        nativeBuildInputs = (with pkgs;[  ]) ++ macPackages ++ linuxPackages;
        src = ./.;
      };
    }
  );
}
