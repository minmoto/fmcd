{
  description = "A fedimint client daemon for server side applications to hold, use, and manage Bitcoin and ecash";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-24.05";

    flakebox = {
      url = "github:rustshop/flakebox?rev=ee39d59b2c3779e5827f8fa2d269610c556c04c8";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";

    fedimint.url = "github:fedimint/fedimint?ref=v0.4.2";
  };

  outputs =
    {
      self,
      nixpkgs,
      flakebox,
      flake-utils,
      fedimint,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = fedimint.overlays.fedimint;
        };

        lib = pkgs.lib;
        flakeboxLib = flakebox.lib.${system} { };

        # Source files for the build
        rustSrc = flakeboxLib.filterSubPaths {
          root = builtins.path {
            name = "fmcd";
            path = ./.;
          };
          paths = [
            "Cargo.toml"
            "Cargo.lock"
            ".cargo"
            "src"
          ];
        };

        # Build configuration
        commonArgs = {
          buildInputs =
            [ ]
            ++ lib.optionals pkgs.stdenv.isDarwin [ pkgs.darwin.apple_sdk.frameworks.SystemConfiguration ];
          nativeBuildInputs = [ pkgs.pkg-config ];
        };

        # Toolchain configuration
        toolchainArgs = {
          extraRustFlags = "--cfg tokio_unstable";
          components = [
            "rustc"
            "cargo"
            "clippy"
            "rust-analyzer"
            "rust-src"
          ];
        };

        toolchainsStd = flakeboxLib.mkStdFenixToolchains toolchainArgs;

        # Build outputs
        outputs = (flakeboxLib.craneMultiBuild { toolchains = toolchainsStd; }) (
          craneLib':
          let
            craneLib =
              (craneLib'.overrideArgs {
                pname = "fmcd";
                src = rustSrc;
              }).overrideArgs
                commonArgs;
          in
          rec {
            workspaceDeps = craneLib.buildDepsOnly { };

            fmcd = craneLib.buildPackage {
              pname = "fmcd";
              cargoArtifacts = workspaceDeps;
            };

            fmcd-oci = pkgs.dockerTools.buildLayeredImage {
              name = "fmcd";
              contents = [ fmcd ];
              config = {
                Cmd = [ "${fmcd}/bin/fmcd" ];
              };
            };
          }
        );
      in
      {
        packages = {
          default = outputs.fmcd;
          oci = outputs.fmcd-oci;
        };

        devShells = flakeboxLib.mkShells {
          packages = [ ];
          buildInputs = commonArgs.buildInputs ++ [ pkgs.glibcLocales ];
          nativeBuildInputs =
            with pkgs;
            [
              mprocs
              bitcoind
              clightning
              lnd
              esplora-electrs
              electrs
              pkg-config
              perl
            ]
            ++ [
              fedimint.packages.${system}.devimint
              fedimint.packages.${system}.gateway-pkgs
              fedimint.packages.${system}.fedimint-pkgs
            ];
          shellHook = ''
            export RUSTFLAGS="--cfg tokio_unstable"
            export RUSTDOCFLAGS="--cfg tokio_unstable"
            export RUST_LOG="info"
            export LOCALE_ARCHIVE="${pkgs.glibcLocales}/lib/locale/locale-archive"
            export LANG="en_US.UTF-8"
            export LC_ALL="en_US.UTF-8"
          '';
        };
      }
    );
}
