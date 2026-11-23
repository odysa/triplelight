#!/usr/bin/env bash
# S1 sidecar (docs/eval-plan/PLAN.md "S1. Memory-limited queries"; owner Alex): samples the cgroup of one running server
# every 0.5 s until killed. session.sh starts it when S1_SAMPLE=1. Linux only; elsewhere it records why and exits.
#   s1-sample.sh <system> <out.tsv>
# Columns: epoch, memory.current, memory.peak, anon, file, pgmajfault, device bytes read (io.stat rbytes, all devices),
#          oom_kill (memory.events). The server's cgroup: its Docker container's, or the bench-<system> scope that
#          lib.sh start_system creates for native servers.
set -u
SYS=$1; OUTF=$2
HERE=$(cd "$(dirname "$0")" && pwd); . "$HERE/lib.sh"
if [ "$(uname)" != Linux ] || [ ! -f /sys/fs/cgroup/cgroup.controllers ]; then echo "# no cgroup v2 here: S1 sampling skipped" > "$OUTF"; exit 0; fi
cg() {
  local id
  case $SYS in
    qlever|qlever-two-order) id=$(docker inspect -f '{{.Id}}' bench-qlever 2>/dev/null);;
    virtuoso) id=$(docker inspect -f '{{.Id}}' bench-virtuoso 2>/dev/null);;
    rdf4j-native|rdf4j-native-default) id=$(docker inspect -f '{{.Id}}' bench-rdf4j 2>/dev/null);;
    millenniumdb) id=$(docker inspect -f '{{.Id}}' bench-mdb 2>/dev/null);;
    *) echo "/sys/fs/cgroup/system.slice/bench-$SYS.scope"; return;;
  esac
  for d in /sys/fs/cgroup/system.slice/docker-$id.scope /sys/fs/cgroup/docker/$id; do [ -d "$d" ] && { echo "$d"; return; }; done
}
CG=${S1_CGROUP:-$(cg)}   # S1_CGROUP overrides the lookup (tests)
[ -n "$CG" ] && [ -f "$CG/memory.current" ] || { echo "# no cgroup found for $SYS: the S1 run is unmeasured" > "$OUTF"; echo "s1-sample: no cgroup for $SYS" >&2; exit 1; }
printf '# cgroup %s\n# epoch\tcurrent\tpeak\tanon\tfile\tpgmajfault\trbytes\toom_kill\n' "$CG" > "$OUTF"
while [ -f "$CG/memory.current" ]; do
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(date +%s.%N)" "$(cat "$CG/memory.current")" "$(cat "$CG/memory.peak" 2>/dev/null)" \
    "$(awk '$1=="anon"{print $2}' "$CG/memory.stat")" "$(awk '$1=="file"{print $2}' "$CG/memory.stat")" \
    "$(awk '$1=="pgmajfault"{print $2}' "$CG/memory.stat")" \
    "$(awk '{for(i=2;i<=NF;i++) if ($i ~ /^rbytes=/) {split($i,a,"="); s+=a[2]}} END {print s+0}' "$CG/io.stat" 2>/dev/null)" \
    "$(awk '$1=="oom_kill"{print $2}' "$CG/memory.events")" >> "$OUTF"
  sleep 0.5
done
