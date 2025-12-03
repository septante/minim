{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      overlays = [
        (import rust-overlay)
        (_: prev: {
          rust-toolchain = prev.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        })
      ];

      systems = [
        "x86_64-linux"
      ];

      forAllSystems =
        f: nixpkgs.lib.genAttrs systems (system: f { pkgs = import nixpkgs { inherit system overlays; }; });

      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
    in
    {
      formatter = forAllSystems ({ pkgs }: pkgs.nixfmt-tree);

      devShells = forAllSystems (
        { pkgs }:
        with pkgs;
        {
          default = mkShell rec {
            buildInputs = [
              # rust-bin.stable.latest.default
              rust-toolchain
              rust-analyzer

              # Needed for rodio to work with ALSA
              pkg-config
              alsa-lib
            ];

            LD_LIBRARY_PATH = "${lib.makeLibraryPath buildInputs}";
          };
        }
      );

      packages = forAllSystems (
        { pkgs }:
        with pkgs;
        {
          default = rustPlatform.buildRustPackage {
            pname = "minim";
            src = lib.cleanSource ./.;
            inherit version;

            cargoLock = {
              lockFile = ./Cargo.lock;
              outputHashes = {
                "nucleo-0.5.0" = "sha256-AiaIQ2q2UyFd1Cqlcz4AVgHW+FIY/xp6/Z8XEGhnlok=";
              };
            };

            nativeBuildInputs = [
              pkg-config
            ];

            buildInputs = [
              alsa-lib
            ];
          };
        }
      );
    };
}
