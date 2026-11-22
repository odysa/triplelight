#!/usr/bin/env bash
# Prepare a TOOLS directory for session.sh without touching a shared one: links to the inputs and binaries, and BSBM-1M
# generated with the BSBM tools. Usage: prep.sh <tools-dir> <source-tools-dir>   (source has lubm10.nt, the Jena and
# Fuseki distributions, the oxigraph binary and bsbm/bsbmtools-0.2)
set -eu
T=$1; S=$2; mkdir -p "$T"
# lubm10.nt is copied, not linked: the Docker systems mount only $T, so a link to $S would dangle inside the container.
[ -e "$T/lubm10.nt" ] || cp "$S/lubm10.nt" "$T/lubm10.nt"
for f in oxigraph apache-jena-6.2.0 apache-jena-fuseki-6.2.0; do [ -e "$T/$f" ] || ln -s "$S/$f" "$T/$f"; done
if [ ! -f "$T/bsbm1.nt" ]; then
  rm -rf "$T/bsbmgen"; cp -R "$S/bsbm/bsbmtools-0.2" "$T/bsbmgen"
  ( cd "$T/bsbmgen" && ./generate -fc -pc 2800 -s nt -fn bsbm1 > "$T/bsbm-generate.log" 2>&1 && mv bsbm1.nt "$T/bsbm1.nt" )
fi
wc -l "$T/lubm10.nt" "$T/bsbm1.nt"
