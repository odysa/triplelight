#!/usr/bin/env bash
# Machine qualification before a scored run (task #130 phase 6; owner Alex). About 3 minutes; `--quick` about 30 s.
# It measures and records, it never tunes (provision.sh tunes); its only write is a temporary file on the data disk.
# Run with no benchmark or build running:
#   machine-check.sh <out-dir> [--data <dir>] [--min-mem-gb N] [--min-read-mbps N] [--min-write-mbps N] [--quick]
#                    [--allow-vm] [--allow-no-cpufreq] [--thp madvise|never|always] [--ignore <regex>]
# --ignore: busy processes whose command matches this extended regex do not fail the idle check (the laptop's window
#   server, say). They are still listed, and the regex is recorded as `idle_ignore`, so a report can see what was waived.
# --allow-no-cpufreq: accept a host whose governor and turbo state cannot be read (recorded as waived); otherwise that
#   fails, since the settings cannot be proven.
# Linux (the rented host): run as root after provision.sh. Checks bare metal, governor `performance` on every core,
# turbo off, THP as expected, no swap, exclusive use (one login, no containers or containerd shims, no process above
# 2% of a core, host-wide busy time at most 0.1 core, steal time at most 0.5%), spin-loop stability on the first and
# last core, and direct-I/O read and write throughput on the data disk.
# macOS (the laptop): the checks that apply there: exclusive use (no process above 2% of a core), no thermal or
# power-source throttling, spin-loop stability, memory. Its receipt says `os darwin`; reports mark those rows.
# Writes <out-dir>/machine-check.txt (one `key value` line per fact, `FAIL <check>: <why>` per failure) and
# <out-dir>/machine-check.json: {passed, timestamp, boot_id, quick, model, cpu, cores, ram_gb, os, kernel, facts,
# failures}. A consumer must reject a receipt that failed, is `quick`, is older than 24 h, or whose boot_id is not the
# current boot's (a reboot can undo the CPU settings). Exit 0 on pass, 3 on a failed check, 2 on a usage error.
set -euo pipefail
OUT=${1:?usage: machine-check.sh <out-dir> [options]}; shift
OS=$(uname | tr 'A-Z' 'a-z')
DATA=${DATA:-/data}; MIN_MEM_GB=0; MIN_READ_MBPS=1000; MIN_WRITE_MBPS=500; QUICK=0; ALLOW_VM=0; ALLOW_NO_CPUFREQ=0
IGNORE=; THP_WANT=madvise
while [ $# -gt 0 ]; do case $1 in
  --data) DATA=$2; shift 2;; --min-mem-gb) MIN_MEM_GB=$2; shift 2;; --min-read-mbps) MIN_READ_MBPS=$2; shift 2;;
  --min-write-mbps) MIN_WRITE_MBPS=$2; shift 2;; --thp) THP_WANT=$2; shift 2;;
  --quick) QUICK=1; shift;; --allow-vm) ALLOW_VM=1; shift;; --allow-no-cpufreq) ALLOW_NO_CPUFREQ=1; shift;;
  --ignore) IGNORE=$2; shift 2;;
  *) echo "unknown option $1" >&2; exit 2;; esac; done
case $OS in linux) [ "$(id -u)" = 0 ] || { echo "machine-check: run as root on Linux (direct I/O, every process's CPU time)" >&2; exit 2; };;
            darwin) ;; *) echo "machine-check: unsupported OS $OS" >&2; exit 2;; esac
# an aborted run must not leave an earlier passing receipt from the same boot behind
mkdir -p "$OUT"; rm -f "$OUT/machine-check.json"; R=$OUT/machine-check.txt; : > "$R"
FAILS=0
rec() { echo "$1 $2" | tee -a "$R"; }
fail() { echo "FAIL $1: $2" | tee -a "$R"; FAILS=$((FAILS + 1)); }
gt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a > b) }'; }
if [ $QUICK = 1 ]; then IDLE_S=10; SPIN_REPS=5; DISK_MB=256; else IDLE_S=60; SPIN_REPS=20; DISK_MB=4096; fi
rec started "$(date -u +%FT%TZ)"; rec quick $QUICK; rec os "$OS"

# --- host ------------------------------------------------------------------------------------------------------------
rec kernel "$(uname -r)"; rec arch "$(uname -m)"
if [ "$OS" = linux ]; then
  rec boot_id "$(cat /proc/sys/kernel/random/boot_id)"
  rec model "\"$(cat /sys/devices/virtual/dmi/id/product_name 2>/dev/null || echo unknown)\""
  rec cpu "\"$(lscpu | awk -F': *' '/^Model name/{print $2; exit}')\""
  rec cores "$(nproc)"
  rec threads_per_core "$(lscpu | awk -F': *' '/^Thread\(s\) per core/{print $2; exit}')"
  rec numa_nodes "$(lscpu | awk -F': *' '/^NUMA node\(s\)/{print $2; exit}')"
  MEM_KB=$(awk '/MemTotal/{print $2}' /proc/meminfo)
else
  rec boot_id "$(sysctl -n kern.boottime | sed -E 's/^[{] sec = ([0-9]+),.*/\1/')"
  rec model "$(sysctl -n hw.model)"
  rec cpu "\"$(sysctl -n machdep.cpu.brand_string)\""
  rec cores "$(sysctl -n hw.ncpu)"
  rec perf_cores "$(sysctl -n hw.perflevel0.physicalcpu 2>/dev/null || echo n/a)"
  MEM_KB=$(( $(sysctl -n hw.memsize) / 1024 ))
fi
RAM_GB=$((MEM_KB / 1048576)); rec ram_gb $RAM_GB
[ $RAM_GB -ge "$MIN_MEM_GB" ] || fail memory "$RAM_GB GB < --min-mem-gb $MIN_MEM_GB"

# --- Linux CPU and memory settings (provision.sh sets them; this proves they held) ---------------------------------------
if [ "$OS" = linux ]; then
  VIRT=$(systemd-detect-virt 2>/dev/null || true); rec virtualization "${VIRT:-none}"
  if [ -n "$VIRT" ] && [ "$VIRT" != none ] && [ $ALLOW_VM = 0 ]; then
    fail virtualization "host is virtualised ($VIRT); scored runs need bare metal (--allow-vm for qualification only)"
  fi
  rec clocksource "$(cat /sys/devices/system/clocksource/clocksource0/current_clocksource 2>/dev/null || echo n/a)"
  THP=$(sed -E 's/.*\[(.*)\].*/\1/' /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || echo n/a); rec thp "$THP"
  [ "$THP" = "$THP_WANT" ] || fail thp "transparent huge pages are '$THP', expected '$THP_WANT' (--thp)"
  # without cpufreq or pstate sysfs the frequency settings cannot be proven, so that fails unless waived
  nocf() { if [ $ALLOW_NO_CPUFREQ = 1 ]; then rec "$1" "n/a (waived by --allow-no-cpufreq)"; else fail "$1" "$2"; fi; }
  GOVS=$(cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor 2>/dev/null | sort | uniq -c | awk '{printf "%s:%s ", $2, $1}' || true)
  rec governors "\"${GOVS:-n/a}\""
  if [ -z "$GOVS" ]; then nocf governor "no cpufreq governor in sysfs (--allow-no-cpufreq to waive)"
  elif [ "$(cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor | sort -u)" != performance ]; then
    fail governor "not every core is on performance ($GOVS)"
  fi
  if [ -r /sys/devices/system/cpu/intel_pstate/no_turbo ]; then
    T=$(cat /sys/devices/system/cpu/intel_pstate/no_turbo); rec no_turbo "$T"; [ "$T" = 1 ] || fail turbo "intel_pstate turbo is on"
  elif [ -r /sys/devices/system/cpu/cpufreq/boost ]; then
    T=$(cat /sys/devices/system/cpu/cpufreq/boost); rec boost "$T"; [ "$T" = 0 ] || fail turbo "cpufreq boost is on"
  else nocf turbo "no intel_pstate or cpufreq boost control in sysfs (--allow-no-cpufreq to waive)"; fi
  SWAP_KB=$(awk '/SwapTotal/{print $2}' /proc/meminfo); rec swap_kb "$SWAP_KB"
  [ "$SWAP_KB" = 0 ] || fail swap "swap is configured (${SWAP_KB} kB)"
else
  THERM=$(pmset -g therm 2>/dev/null | awk -F'= *' '/CPU_Speed_Limit/{print $2}' || true); rec cpu_speed_limit "${THERM:-n/a}"
  [ -z "$THERM" ] || [ "$THERM" = 100 ] || fail thermal "CPU speed limit is $THERM%"
  PWR=$(pmset -g batt 2>/dev/null | head -1 | sed -E "s/.*'(.*)'.*/\1/" || true); rec power_source "\"${PWR:-n/a}\""
  [ "$PWR" != "Battery Power" ] || fail power "on battery power"
  LPM=$(pmset -g 2>/dev/null | awk '/lowpowermode/{print $2}' || true); rec low_power_mode "${LPM:-n/a}"
  [ "${LPM:-0}" = 0 ] || fail power "low power mode is on"
fi

# --- exclusive use ------------------------------------------------------------------------------------------------------
if [ "$OS" = linux ]; then
  USERS=$(who | awk '{print $1}' | sort -u | tr '\n' ' '); rec logins "\"${USERS}\""
  [ "$(who | awk '{print $1}' | sort -u | wc -l)" -le 1 ] || fail logins "more than one user is logged in ($USERS)"
fi
# Docker and Podman containers, plus any containerd shim (a container started by another runtime still has one)
CONTAINERS=0
for rt in docker podman; do command -v $rt >/dev/null && CONTAINERS=$((CONTAINERS + $($rt ps -q 2>/dev/null | wc -l))); done
SHIMS=$(pgrep -f containerd-shim 2>/dev/null | wc -l | tr -d ' ' || true)
rec running_containers "$CONTAINERS"; rec containerd_shims "${SHIMS:-0}"
[ "$CONTAINERS" = 0 ] || fail containers "$CONTAINERS containers are running"
[ "${SHIMS:-0}" = 0 ] || [ "$CONTAINERS" != 0 ] || fail containers "${SHIMS} containerd shims are running"
# CPU seconds per process over the idle window: clock ticks from /proc on Linux (the command name may hold spaces, so
# fields are counted after its closing parenthesis), `ps` cputime on macOS
snap() {
  if [ "$OS" = linux ]; then
    local hz f s pid; hz=$(getconf CLK_TCK)
    for f in /proc/[0-9]*/stat; do
      s=$(cat "$f" 2>/dev/null) || continue; pid=${s%% *}
      echo "$pid ${s##*) }" | awk -v hz="$hz" '{ print $1, ($13 + $14) / hz }'
    done | sort
  else
    ps -A -o pid=,time= | awk '{ n = split($2, a, /[-:]/); s = 0; d = 0
      if ($2 ~ /-/) { d = a[1]; for (i = 2; i <= n; i++) s = s * 60 + a[i] } else for (i = 1; i <= n; i++) s = s * 60 + a[i]
      print $1, d * 86400 + s }' | sort
  fi
}
# host-wide ticks (total, steal, busy = user+nice+system+irq+softirq): short-lived processes the per-process scan misses
# still show up in busy
host() { if [ "$OS" = linux ]; then awk '/^cpu /{printf "%d %d %d\n", $2+$3+$4+$5+$6+$7+$8+$9, $9, $2+$3+$4+$7+$8}' /proc/stat; else echo "0 0 0"; fi; }
# host totals are read inside the two per-process snapshots, so the snapshots' own work is outside the window
A=$(snap); read -r T0 S0 U0 < <(host); sleep "$IDLE_S"; read -r T1 S1 U1 < <(host); B=$(snap)
# a process started during the window counts from zero
BUSY=$(printf '%s\n' "$A" "--" "$B" | awk -v w="$IDLE_S" '$1 == "--" { b = 1; next } !b { a[$1] = $2; next }
  { pct = ($2 - a[$1]) * 100 / w; if (pct > 2) printf "%s:%.0f%% ", $1, pct }')
rec idle_ignore "\"${IGNORE:-none}\""
COUNTED=
for p in $BUSY; do
  CMD=$(ps -o comm= -p "${p%%:*}" 2>/dev/null || true)
  if [ -n "$IGNORE" ] && printf '%s' "$CMD" | grep -Eq -- "$IGNORE"; then rec busy_process_ignored "\"$p ${CMD##*/}\""
  else rec busy_process "\"$p ${CMD##*/}\""; COUNTED="$COUNTED$p "; fi
done
[ -z "$COUNTED" ] || fail idle "processes above 2% of a core over ${IDLE_S}s: $COUNTED"
rec loadavg "\"$(uptime | sed -E 's/.*load averages?: //')\""
if [ "$OS" = linux ]; then
  STEAL=$(awk -v t=$((T1 - T0)) -v s=$((S1 - S0)) 'BEGIN { printf "%.3f", (t > 0 ? s * 100 / t : 0) }'); rec steal_pct "$STEAL"
  if gt "$STEAL" 0.5; then fail steal "steal time ${STEAL}% over ${IDLE_S}s"; fi
  HOST_CORES=$(awk -v u=$((U1 - U0)) -v hz="$(getconf CLK_TCK)" -v w="$IDLE_S" 'BEGIN { printf "%.3f", u / (hz * w) }')
  rec host_busy_cores "$HOST_CORES"
  if gt "$HOST_CORES" 0.1; then fail idle "the host used $HOST_CORES cores over ${IDLE_S}s while idle (limit 0.1)"; fi
fi

# --- frequency stability: the same fixed spin, repeated, on the first and last core (Linux) or unpinned (macOS) ----------
SPIN='import statistics as s, sys, time
n = int(sys.argv[1]); v = []
for _ in range(n):
    t = time.perf_counter(); x = 0
    for i in range(3_000_000): x += i * i
    v.append(time.perf_counter() - t)
m = s.median(v); print(f"{m:.6f} {(max(v) - min(v)) / m:.4f}")'
if [ "$OS" = linux ]; then
  LAST=$(( $(nproc) - 1 ))
  read -r M0 D0 < <(taskset -c 0 python3 -c "$SPIN" "$SPIN_REPS"); read -r M1 D1 < <(taskset -c "$LAST" python3 -c "$SPIN" "$SPIN_REPS")
  rec spin_core0 "median_s=$M0,spread=$D0"; rec "spin_core$LAST" "median_s=$M1,spread=$D1"
  CROSS=$(awk -v a="$M0" -v b="$M1" 'BEGIN { d = a > b ? a - b : b - a; printf "%.4f", d / (a < b ? a : b) }'); rec spin_cross_core "$CROSS"
  if gt "$D0" 0.03 || gt "$D1" 0.03; then fail frequency "spin spread above 3% on core 0 or $LAST ($D0, $D1)"; fi
  if gt "$CROSS" 0.03; then fail frequency "cores 0 and $LAST differ by more than 3% ($CROSS)"; fi
else  # macOS cannot pin a thread to a core; the scheduler puts a busy thread on a performance core
  read -r M0 D0 < <(python3 -c "$SPIN" "$SPIN_REPS"); rec spin "median_s=$M0,spread=$D0"
  if gt "$D0" 0.05; then fail frequency "spin spread above 5% ($D0)"; fi
fi

# --- data disk (Linux): direct-I/O sequential write then read (O_DIRECT bypasses the page cache, so nothing is dropped) ----------------------------------------
if [ "$OS" = linux ]; then
  if [ -d "$DATA" ]; then
    rec data_fs "\"$(df -T "$DATA" | awk 'NR==2{print $1, $2}')\""; rec data_free_gb "$(df -B1G --output=avail "$DATA" | tail -1 | tr -d ' ')"
    F=$DATA/.machine-check.$$; trap 'rm -f "$F"' EXIT
    W=$( { dd if=/dev/zero of="$F" bs=1M count="$DISK_MB" oflag=direct conv=fsync 2>&1 >/dev/null; } | awk '/copied/{print $(NF-1), $NF}')
    RD=$( { dd if="$F" of=/dev/null bs=1M iflag=direct 2>&1 >/dev/null; } | awk '/copied/{print $(NF-1), $NF}')
    rm -f "$F"
    rec disk_write "\"$W\""; rec disk_read "\"$RD\""
    mbps() { echo "$1" | awk '{ v = $1; if ($2 ~ /^GB/) v *= 1000; else if ($2 ~ /^kB/) v /= 1000; printf "%d", v }'; }
    RMB=$(mbps "$RD"); WMB=$(mbps "$W")
    [ "${RMB:-0}" -ge "$MIN_READ_MBPS" ] || fail disk "direct read ${RMB:-0} MB/s < --min-read-mbps $MIN_READ_MBPS"
    [ "${WMB:-0}" -ge "$MIN_WRITE_MBPS" ] || fail disk "direct write ${WMB:-0} MB/s < --min-write-mbps $MIN_WRITE_MBPS"
  else fail disk "data directory $DATA is missing (run provision.sh first)"; fi
else rec disk "\"not measured on macOS (no direct I/O)\""; fi

# --- verdict ------------------------------------------------------------------------------------------------------------
rec timestamp "$(date -u +%FT%TZ)"
if [ $FAILS = 0 ]; then rec verdict pass; else rec verdict fail; fi
python3 - "$R" "$OUT/machine-check.json" <<'EOF'
import json, sys
facts, failures = {}, []
for line in open(sys.argv[1]):
    line = line.rstrip("\n")
    if line.startswith("FAIL "):
        check, _, why = line[5:].partition(": ")
        failures.append({"check": check, "why": why})
        continue
    key, _, val = line.partition(" ")
    if len(val) >= 2 and val[0] == val[-1] == '"': val = val[1:-1]   # rec quotes values that hold spaces
    facts[key] = facts[key] + "; " + val if key in facts else val
top = {k: facts.get(k) for k in ("timestamp", "boot_id", "model", "cpu", "os", "kernel")}
doc = {"schema": "machine-check-1", "passed": not failures and facts.get("verdict") == "pass", **top,
       "quick": facts.get("quick") == "1", "cores": int(facts["cores"]), "ram_gb": int(facts["ram_gb"]),
       "facts": facts, "failures": failures}
json.dump(doc, open(sys.argv[2], "w"), indent=1)
EOF
[ $FAILS = 0 ] || exit 3
