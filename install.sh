#!/bin/sh
# Installs salir, as `make` built it in dist/: the program and CEF's files (copied)
# in $PREFIX/lib/salir/, a link to it in $PREFIX/bin/, the plugin header in
# $PREFIX/include/. PREFIX: ~/.local unless set.
#
#   ./install.sh                 (make install)
#   ./install.sh --uninstall     (make uninstall)
set -e
cd "$(dirname "$0")"
PREFIX="${PREFIX:-$HOME/.local}"
lib="$PREFIX/lib/salir"

if [ "${1:-}" = --uninstall ]; then
  rm -rf "$lib"
  rm -f "$PREFIX/bin/salir" "$PREFIX/include/salir.h"
  echo "salir removed from $PREFIX"
  exit 0
fi

[ -x dist/salir ] || { echo "No dist/salir: run make first" >&2; exit 1; }
# Put together beside it, then swapped in (a salir running meanwhile keeps its files).
rm -rf "$lib.new"
mkdir -p "$lib.new" "$PREFIX/bin" "$PREFIX/include"
for f in dist/*; do
  case "$(basename "$f")" in
    salir-particles*|plugins|extensions) ;; # (the examples: not the engine; extensions/: CEF makes it as it runs)
    *) cp -rL "$f" "$lib.new/" ;;
  esac
done
rm -rf "$lib.old"
[ -d "$lib" ] && mv "$lib" "$lib.old"
mv "$lib.new" "$lib"
rm -rf "$lib.old"
ln -sfn "$lib/salir" "$PREFIX/bin/salir"
cp include/salir.h "$PREFIX/include/salir.h"
echo "salir installed: $PREFIX/bin/salir ($(du -sh "$lib" | cut -f1) in $lib)"
