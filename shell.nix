{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  packages = with pkgs; [
    bun
    nodejs_24
    cargo
    rustc
    rustfmt
    clippy
  ];
}
