{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  packages = with pkgs; [
    bun
    nodejs_24
    rustc
    clippy
  ];
}
