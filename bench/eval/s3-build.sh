#!/usr/bin/env bash
# S3 bulk build (docs/eval-plan/PLAN.md "S3. Bulk build"; owner Alex): one load of one system on one dataset, with the
# loader exactly as `systems.sh <system> load` runs it, measured from outside.
#   s3-build.sh <system> <dataset> <tag> [--mem GB] [--cpus N]
# Writes $OUT/<system>-<dataset>-<tag>/{build.log,samples.tsv,receipt.json}. OUT defaults to bench/results/s3.
#
# What is measured, every 2 s while the loader runs:
#   - the store directory's size (du), which includes the loader's scratch when it writes it there, and free space on its
#     volume; the receipt keeps the peak and the final size;
#   - memory: on Linux, the cgroup of the loader: the systemd scope this script starts (native loaders) and the Docker
#     container's own cgroup (Docker loaders): memory.peak, and memory.stat anon/file at the sample where anon+file peaks
#     (page cache counts against memory.max, so both are kept); elsewhere, RSS of the process tree plus `docker stats`
#     memory, labelled as such;
#   - device writes (Linux): /proc/diskstats sectors written on the store's device, before and after.
# Wall, user and system CPU come from the time utility around `systems.sh` (for Docker loaders that is the client, so the
# receipt also keeps the container's cpu.stat on Linux). The page cache is dropped first (lib.sh drop_caches).
# On a laptop this is a dry run of the same path: numbers are correctness evidence only, never timing claims.
set -eu
SYS=$1; D=$2; TAG=$3; shift 3
MEM=12; CPUS=8
while [ $# -gt 0 ]; do case $1 in --mem) MEM=$2; shift 2;; --cpus) CPUS=$2; shift 2;; *) echo "unknown option $1" >&2; exit 2;; esac; done
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
T=${TOOLS:-$ROOT/bench/tools}
OUT=${OUT:-$ROOT/bench/results/s3}/$SYS-$D-$TAG
mkdir -p "$OUT"; : > "$OUT/samples.tsv"
. "$HERE/lib.sh"
. "$HERE/VERSIONS"

case $SYS in   # where each loader writes its store (systems.sh)
  qlever) STORE=$T/qlever-$D ;; qlever-two-order) STORE=$T/qlever-two-order-$D ;;
  *) STORE=$T/$SYS-$D ;;
esac
case $SYS in   # the container a Docker loader runs in (empty: native loader)
  qlever|qlever-two-order) CNAME_FILTER="ancestor=$QLEVER_IMAGE" ;;
  virtuoso) CNAME_FILTER="name=bench-virtuoso-load" ;;
  rdf4j-native|rdf4j-native-default) CNAME_FILTER="name=bench-rdf4j-load" ;;
  millenniumdb) CNAME_FILTER="ancestor=mdb" ;;
  *) CNAME_FILTER= ;;
esac
LINUX=0; [ "$(uname)" = Linux ] && [ -f /sys/fs/cgroup/cgroup.controllers ] && LINUX=1
dev_written() {   # sectors written on the device that holds $T (Linux), else 0
  [ $LINUX = 1 ] || { echo 0; return; }
  local dev; dev=$(basename "$(df --output=source "$T" | tail -1)")
  awk -v d="$dev" '$3==d {print $10}' /proc/diskstats
}
cg_of_container() {   # cgroup v2 directory of a running container, or nothing
  local id; id=$(docker ps -q --no-trunc --filter "$CNAME_FILTER" | head -1); [ -n "$id" ] || return 0
  for d in /sys/fs/cgroup/system.slice/docker-$id.scope /sys/fs/cgroup/docker/$id; do [ -d "$d" ] && { echo "$d"; return; }; done
}
tree_rss_kb() {   # RSS of a process and all its descendants, in KiB
  ps -Ao pid=,ppid=,rss= | awk -v root="$1" '{p[$1]=$2; r[$1]=$3} END {s=0; for (q in r) {x=q; while (x != "" && x != root && x != 1 && x != 0) x=p[x]; if (x == root) s+=r[q]} print s}'
}

echo "$(drop_caches)" > "$OUT/drop_caches.txt"
W0=$(dev_written)
SCOPE=s3-$SYS-$D-$TAG-$$
if [ $LINUX = 1 ] && command -v systemd-run >/dev/null; then
  # The native loaders are charged to this scope; Docker loaders to their container's cgroup (sampled below).
  RUN=(systemd-run --scope --quiet --unit="$SCOPE" -p MemoryMax="${MEM}G" -p MemorySwapMax=0 --)
  TIMEFMT=(/usr/bin/time -v)
else
  RUN=(); TIMEFMT=(/usr/bin/time -l)
fi
START=$(date +%s.%N 2>/dev/null || date +%s)
"${TIMEFMT[@]}" "${RUN[@]}" "$HERE/systems.sh" "$SYS" load "$D" --mem "$MEM" --cpus "$CPUS" > "$OUT/build.log" 2>&1 &
PID=$!
CG=
while kill -0 $PID 2>/dev/null; do
  s=$(du -sk "$STORE" 2>/dev/null | cut -f1); f=$(df -k "$T" | awk 'NR==2{print $4}')
  mem_kb=; anon=; file=; peak=
  if [ $LINUX = 1 ]; then
    [ -n "$CNAME_FILTER" ] && [ -z "$CG" ] && CG=$(cg_of_container)
    [ -z "$CNAME_FILTER" ] && CG=/sys/fs/cgroup/system.slice/$SCOPE.scope
    if [ -n "$CG" ] && [ -f "$CG/memory.peak" ]; then
      peak=$(cat "$CG/memory.peak"); anon=$(awk '$1=="anon"{print $2}' "$CG/memory.stat"); file=$(awk '$1=="file"{print $2}' "$CG/memory.stat")
      cat "$CG/cpu.stat" > "$OUT/cpu.stat.last" 2>/dev/null || true
    fi
  else
    mem_kb=$(tree_rss_kb $PID)
    if [ -n "$CNAME_FILTER" ]; then
      c=$(docker stats --no-stream --format '{{.MemUsage}}' $(docker ps -q --filter "$CNAME_FILTER") 2>/dev/null | head -1 | awk '{print $1}')
      [ -n "$c" ] && mem_kb="$mem_kb+docker:$c"
    fi
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(date -u +%H:%M:%S)" "${s:-0}" "$f" "${peak:-}" "${anon:-}" "${file:-}" "${mem_kb:-}" >> "$OUT/samples.tsv"
  sleep 2
done
set +e; wait $PID; RC=$?; set -e
END=$(date +%s.%N 2>/dev/null || date +%s)
W1=$(dev_written)
FINAL=$(du -sk "$STORE" 2>/dev/null | cut -f1)
python3 -I - "$OUT" "$SYS" "$D" "$TAG" "$MEM" "$CPUS" "$RC" "$START" "$END" "$W0" "$W1" "${FINAL:-0}" "$LINUX" <<'EOF'
import sys, json, re, os
out, sys_, d, tag, mem, cpus, rc, start, end, w0, w1, final, linux = sys.argv[1:]
rows = [l.rstrip("\n").split("\t") for l in open(os.path.join(out, "samples.tsv")) if l.strip()]
num = lambda x: int(x) if x and x.isdigit() else None
peak_dir = max([num(r[1]) or 0 for r in rows], default=0)
min_free = min([num(r[2]) for r in rows if num(r[2]) is not None], default=None)
mem_peak = max([num(r[3]) for r in rows if num(r[3]) is not None], default=None)
at = max(rows, key=lambda r: (num(r[4]) or 0) + (num(r[5]) or 0), default=None)
log = open(os.path.join(out, "build.log"), errors="replace").read()
def grab(pat):
    m = re.search(pat, log); return float(m.group(1)) if m else None
rec = {
    "system": sys_, "dataset": d, "tag": tag, "mem_gb": int(mem), "cpus": int(cpus), "exit": int(rc),
    "wall_s": round(float(end) - float(start), 2),
    "user_s": grab(r"User time \(seconds\): ([\d.]+)") or grab(r"([\d.]+) user"),
    "sys_s": grab(r"System time \(seconds\): ([\d.]+)") or grab(r"([\d.]+) sys"),
    "client_max_rss_kb": grab(r"Maximum resident set size \(kbytes\): (\d+)") or (lambda b: b / 1024 if b else None)(grab(r"(\d+)\s+maximum resident set size")),
    "cgroup_memory_peak_bytes": mem_peak, "anon_at_peak_bytes": num(at[4]) if at else None, "file_at_peak_bytes": num(at[5]) if at else None,
    "peak_store_dir_kib": peak_dir, "final_store_kib": int(final), "min_free_kib": min_free,
    "device_written_bytes": (int(w1) - int(w0)) * 512 if linux == "1" else None,
    "memory_source": "cgroup" if linux == "1" else "process-tree RSS + docker stats (laptop dry run)",
    "page_cache": open(os.path.join(out, "drop_caches.txt")).read().strip(),
}
json.dump(rec, open(os.path.join(out, "receipt.json"), "w"), indent=1)
print(json.dumps(rec))
EOF
exit $RC
