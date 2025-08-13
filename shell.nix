# Shell environment that uses the flake's devShell
{
  pkgs ? import <nixpkgs> { },
}:
let
  flake = builtins.getFlake (toString ./.);
  system = pkgs.system;
in
flake.devShells.${system}.default or pkgs.mkShell {
  buildInputs = with pkgs; [
    cargo
    rustc
    pkg-config
    openssl
    clang
    glibcLocales
    perl
  ];

  shellHook = ''
    export LOCALE_ARCHIVE="${pkgs.glibcLocales}/lib/locale/locale-archive"
    export LANG="en_US.UTF-8"
    export LC_ALL="en_US.UTF-8"
  '';
}
