{ pkgs ? import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/1306659b587dc277866c7b69eb97e5f07864d8c4.tar.gz") {} }:

with pkgs;
mkShell {
  nativeBuildInputs = [ libiconv pkg-config fontconfig freetype openssl ];
}
