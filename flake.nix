{
  description = "A fedimint client daemon for server side applications to hold, use, and manage Bitcoin and ecash";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.05";

    flakebox = {
      url = "github:rustshop/flakebox?rev=f90159e9c8e28a8a12e8d8673e37e80ef1a10c08";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";

    fedimint.url = "github:fedimint/fedimint?ref=v0.8.0";
  };

  outputs =
    {
      self,
      nixpkgs,
      flakebox,
      flake-utils,
      fedimint,
    }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
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
          buildInputs = [ ];
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
          # Use latest stable Rust to support edition2024
          channel = "stable";
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

            oci = pkgs.dockerTools.buildLayeredImage {
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
          oci = outputs.oci;
        };

        devShells = {
          default = flakeboxLib.mkDevShell {
            buildInputs = commonArgs.buildInputs ++ [ pkgs.glibcLocales ];
            nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
              # Build tools
              pkgs.perl
              pkgs.clang
              pkgs.llvmPackages.libclang

              # Development tools
              pkgs.mprocs
            ];
            shellHook = ''
              export RUSTFLAGS="--cfg tokio_unstable"
              export RUSTDOCFLAGS="--cfg tokio_unstable"
              export RUST_LOG="info"
              export LOCALE_ARCHIVE="${pkgs.glibcLocales}/lib/locale/locale-archive"
              export LANG="en_US.UTF-8"
              export LC_ALL="en_US.UTF-8"
              export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
            '';
          };
        };
      }
    );
}
