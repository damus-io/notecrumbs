{ pkgs ? import <nixpkgs> {} }:
with pkgs;
mkShell {
  nativeBuildInputs = [ gdb cargo rustc rustfmt libiconv pkg-config fontconfig freetype ];
}
