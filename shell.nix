{ pkgs ? import <nixpkgs> {} }:
with pkgs;
mkShell {
  nativeBuildInputs = [ libiconv pkg-config fontconfig freetype ];
}
