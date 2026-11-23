#!/usr/bin/env bash
# Day-one provisioning of the dedicated benchmark machine (docs/eval-plan/RUNBOOK.md; owner Alex). Run as root on a fresh
# Debian trixie host. Idempotent: every step checks before it acts, so a rerun after a failure resumes. Every step and
# every version, checksum and setting lands in $DATA/receipts/provision.txt; $DATA/eval.env holds the paths for later
# shells (`. /data/eval.env`).
#   provision.sh [--device /dev/nvme1n1] [--data /data] [--commit <sha>] [--repo <url|path|bundle>] [--dry-run] [--skip-images]
# --repo: where to clone TripleLight from (default the GitHub repository, which is private: set up access first, or pass
#   a git bundle copied to the machine).
# --dry-run: the laptop check in a privileged container: skip the disk format/mount and CPU tuning, keep the rest.
# --skip-images: skip the Docker image pulls and the MillenniumDB image build (the slowest step; dry runs only).
set -euo pipefail
DEVICE=; DATA=/data; COMMIT=; DRY=0; IMAGES=1; REPO_URL=
while [ $# -gt 0 ]; do case $1 in
  --device) DEVICE=$2; shift 2;; --data) DATA=$2; shift 2;; --commit) COMMIT=$2; shift 2;; --dry-run) DRY=1; shift;;
  --skip-images) IMAGES=0; shift;; --repo) REPO_URL=$2; shift 2;;
  *) echo "unknown option $1" >&2; exit 2;; esac; done
[ "$(id -u)" = 0 ] || { echo "run as root (systemd-run scopes and drop_caches need it)" >&2; exit 2; }
ARCH=$(uname -m)
case $ARCH in x86_64) DARCH=linux_amd64;; aarch64) DARCH=linux_arm64;; *) echo "unsupported arch $ARCH" >&2; exit 2;; esac
# --- pinned downloads (verified before use) --------------------------------------------------------------------------
RUST_TOOLCHAIN=1.100.0
JDK_URL_x86_64=https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.5.1%2B2/OpenJDK25U-jdk_x64_linux_hotspot_25.0.5.1_2.tar.gz
JDK_SHA_x86_64=760f91b22c3efc9bd1b64ff950598231be82928cc807e30691a0e7a2995a62c0
JDK_URL_aarch64=https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.5.1%2B2/OpenJDK25U-jdk_aarch64_linux_hotspot_25.0.5.1_2.tar.gz
JDK_SHA_aarch64=c37879e0d08eeb1baa69fcbb7758cd455b7b1c0ceab1e2a136ccb1713c530984
JENA_URL=https://repo1.maven.org/maven2/org/apache/jena/apache-jena/6.2.0/apache-jena-6.2.0.tar.gz   # same bytes as archive.apache.org, which is slow
JENA_SHA512=2b467d2ff940c207aea935e012ecbb0f0ab3e6bf59b3fc4b3fbc6cf9cea11f1428e97882ec078a5ffc57e114e0f5c88ea98c4575e687cc1dd232f99324b7b2a2
FUSEKI_URL=https://repo1.maven.org/maven2/org/apache/jena/apache-jena-fuseki/6.2.0/apache-jena-fuseki-6.2.0.tar.gz
FUSEKI_SHA512=ba65f5867d2d4741b2ed9e2af5a0d4fbb447909894ab2a0c6bc4dac8997f4fe339c87b13c48d45d054977769f0f8bf763ea346b1f7792d5cdc458041bd43a132
OXIGRAPH_URL_x86_64=https://github.com/oxigraph/oxigraph/releases/download/v0.5.13/oxigraph_v0.5.13_x86_64_linux_gnu
OXIGRAPH_SHA_x86_64=05f54af6d7ebd73888a600bcf25017ef304c51726428f6dae10d32e9185618be
OXIGRAPH_URL_aarch64=https://github.com/oxigraph/oxigraph/releases/download/v0.5.13/oxigraph_v0.5.13_aarch64_linux_gnu
OXIGRAPH_SHA_aarch64=9d3f6ee2e78a3914d563c6109041eab7ecbc6e1aec5b8db285479f7f312ef55d
BLAZEGRAPH_URL=https://repo1.maven.org/maven2/com/blazegraph/blazegraph-jar/2.1.5/blazegraph-jar-2.1.5.jar
BLAZEGRAPH_SHA=8f38b1dae5ad92b0c786b92d1f260a18a4ca64151b62c8d5017dd59b743e131e   # Maven Central sha1 96b876ab… matches
RDF_FUSION_VERSION=0.3.1   # crate rdf-fusion-cli, built --locked with $RUST_TOOLCHAIN
# Signed DuckDB community extension `rdf` for DuckDB 2.0.1, installed by DuckDB itself with signature checks on, then
# compared with this hash of the installed file (fetched 2026-11-23 from community-extensions.duckdb.org/v2.0.1/<arch>/).
DUCKDB_RDF_SHA_linux_amd64=ce9422545435945979ef489bd51f40c43ef188b8d05ac024d880017827906cfe
DUCKDB_RDF_SHA_linux_arm64=ce15ec9e46ed045205a1cf5c28a6cb977a90247b376b1c00fe4f035375a70a5d
REPO_URL=${REPO_URL:-https://github.com/odysa/triplelight.git}

R=$DATA/receipts/provision.txt
step() { echo; echo "== $*"; mkdir -p "$(dirname "$R")"; echo "== $(date -u +%FT%TZ) $*" >> "$R"; }
note() { echo "$*"; echo "   $*" >> "$R"; }
die() { echo "provision: $*" >&2; echo "   FAILED: $*" >> "$R"; exit "${2:-3}"; }
fetch() {  # fetch <url> <dest> <sha256|sha512> <expected>: download once (atomically), verify every time
  if [ ! -f "$2" ]; then
    curl -fsSL --retry 3 -o "$2.part" "$1" || { rm -f "$2.part"; die "download failed: $1"; }
    mv "$2.part" "$2"
  fi
  local got; got=$("${3}sum" "$2" | cut -d' ' -f1)
  [ "$got" = "$4" ] || { rm -f "$2"; die "checksum mismatch for $2: got $got, want $4"; }
  note "$(basename "$2") $3 ok"
}

# --- 1. data disk ------------------------------------------------------------------------------------------------------
step "data disk ${DEVICE:-none} -> $DATA"
if [ $DRY = 1 ]; then mkdir -p "$DATA"; note "dry run: no format/mount"
elif mountpoint -q "$DATA"; then note "already mounted: $(findmnt -no SOURCE,FSTYPE,OPTIONS "$DATA")"
else
  [ -b "$DEVICE" ] || { lsblk >&2; die "--device must be the instance's local NVMe (lsblk above)" 2; }
  blkid "$DEVICE" >/dev/null 2>&1 || mkfs.ext4 -q -E nodiscard -m 0 "$DEVICE"
  mkdir -p "$DATA"; mount -o noatime "$DEVICE" "$DATA"
fi
note "$(df -h "$DATA" | tail -1)"
mkdir -p "$DATA"/{tools,receipts,results,stores,scratch/duckdb,scratch/triplelight,datasets}

# --- 2. CPU and kernel settings -----------------------------------------------------------------------------------------
step "CPU frequency and kernel"
if [ $DRY = 0 ]; then
  for g in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do [ -w "$g" ] && echo performance > "$g"; done
  [ -w /sys/devices/system/cpu/intel_pstate/no_turbo ] && echo 1 > /sys/devices/system/cpu/intel_pstate/no_turbo
  [ -w /sys/devices/system/cpu/cpufreq/boost ] && echo 0 > /sys/devices/system/cpu/cpufreq/boost
  sysctl -qw vm.swappiness=1
  swapoff -a
fi
note "governor: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo n/a)"
note "no_turbo: $(cat /sys/devices/system/cpu/intel_pstate/no_turbo 2>/dev/null || echo n/a); boost: $(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null || echo n/a)"
note "kernel $(uname -r), $ARCH, $(nproc) CPUs, $(awk '/MemTotal/{print $2" kB"}' /proc/meminfo), swap $(awk '/SwapTotal/{print $2" kB"}' /proc/meminfo)"
note "cpu: $(lscpu | awk -F': *' '/^Model name/{print $2; exit}')"
note "cgroup: $(stat -fc %T /sys/fs/cgroup)  (cgroup2fs expected)"
[ "$(stat -fc %T /sys/fs/cgroup)" = cgroup2fs ] || die "cgroup v2 required" 4

# --- 3. packages ----------------------------------------------------------------------------------------------------------
step "packages"
export DEBIAN_FRONTEND=noninteractive
pkgs=(curl ca-certificates git build-essential pkg-config libssl-dev cmake python3 python3-venv python3-dev time procps lsof jq
      iproute2 unzip bzip2 lbzip2 gawk wamerican maven libboost-dev docker.io)
apt-cache show docker-cli >/dev/null 2>&1 && pkgs+=(docker-cli)   # a separate package on trixie
need=(); for p in "${pkgs[@]}"; do dpkg -s "$p" >/dev/null 2>&1 || need+=("$p"); done
if [ ${#need[@]} -gt 0 ]; then apt-get update -qq; apt-get install -y -qq --no-install-recommends "${need[@]}" >/dev/null; fi
note "python $(python3 --version 2>&1 | cut -d' ' -f2), git $(git --version | cut -d' ' -f3), maven $(mvn -B -v 2>/dev/null | head -1 | cut -d' ' -f3), boost $(dpkg -s libboost-dev | awk '/^Version/{print $2}')"

# --- 4. Docker with the systemd cgroup driver ---------------------------------------------------------------------------
step "docker"
systemctl enable --now docker >/dev/null 2>&1 || true
for _ in $(seq 1 30); do docker info >/dev/null 2>&1 && break; sleep 1; done
drv=$(docker info --format '{{.CgroupDriver}} v{{.CgroupVersion}}')
note "docker $(docker version --format '{{.Server.Version}}'), cgroup driver $drv"
[ "$drv" = "systemd v2" ] || die "Docker must use the systemd cgroup driver on cgroup v2 (got $drv)" 4

# --- 5. toolchains --------------------------------------------------------------------------------------------------------
step "toolchains"
export RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
command -v rustup >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none >/dev/null
rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal >/dev/null 2>&1
note "$(rustc +$RUST_TOOLCHAIN --version)"
if [ ! -x /opt/jdk25/bin/java ]; then
  ju=JDK_URL_$ARCH; js=JDK_SHA_$ARCH
  fetch "${!ju}" /opt/jdk25.tar.gz sha256 "${!js}"
  mkdir -p /opt/jdk25 && tar -xzf /opt/jdk25.tar.gz -C /opt/jdk25 --strip-components=1
fi
export JAVA_HOME=/opt/jdk25 PATH=/opt/jdk25/bin:$PATH
note "$(java -version 2>&1 | head -1)"
V=/opt/eval-venv
[ -x $V/bin/python ] || python3 -m venv $V
$V/bin/pip install -q --disable-pip-version-check --require-hashes --only-binary :all: -r "$(dirname "$0")/requirements-eval.txt"
note "venv $($V/bin/python --version): $($V/bin/pip freeze --disable-pip-version-check | tr '\n' ' ')"

# --- 6. systems (pinned) --------------------------------------------------------------------------------------------------
step "systems"
T=$DATA/tools
if [ ! -d "$T/apache-jena-6.2.0" ]; then fetch "$JENA_URL" "$T/jena.tar.gz" sha512 "$JENA_SHA512"; tar -xzf "$T/jena.tar.gz" -C "$T"; fi
if [ ! -d "$T/apache-jena-fuseki-6.2.0" ]; then fetch "$FUSEKI_URL" "$T/fuseki.tar.gz" sha512 "$FUSEKI_SHA512"; tar -xzf "$T/fuseki.tar.gz" -C "$T"; fi
ou=OXIGRAPH_URL_$ARCH; os=OXIGRAPH_SHA_$ARCH
fetch "${!ou}" "$T/oxigraph" sha256 "${!os}"; chmod +x "$T/oxigraph"
note "$("$T/oxigraph" --version)"
mkdir -p "$T/blazegraph"; fetch "$BLAZEGRAPH_URL" "$T/blazegraph/blazegraph-jar-2.1.5.jar" sha256 "$BLAZEGRAPH_SHA"
if [ ! -x "$T/rdf-fusion/bin/rdf-fusion" ]; then
  cargo +$RUST_TOOLCHAIN install -q --locked --root "$T/rdf-fusion" rdf-fusion-cli --version "$RDF_FUSION_VERSION"
fi
note "rdf-fusion-cli $RDF_FUSION_VERSION sha256 $(sha256sum "$T/rdf-fusion/bin/rdf-fusion" | cut -d' ' -f1)"
# DuckDB rdf: installed from the community repository with signature checks on (the default), into a private
# directory on the NVMe, then hashed against the pin.
EXT=$T/duckdb-ext
$V/bin/python -I - "$EXT" <<'PY' | while read -r l; do note "$l"; done
import sys, duckdb
c = duckdb.connect(config={"extension_directory": sys.argv[1], "allow_unsigned_extensions": "false"})
c.execute("INSTALL rdf FROM community"); c.execute("LOAD rdf")
r = c.execute("SELECT extension_version, install_path FROM duckdb_extensions() WHERE extension_name = 'rdf'").fetchone()
print(f"duckdb {duckdb.__version__} rdf {r[0]} at {r[1]}")
PY
f=$(find "$EXT" -name rdf.duckdb_extension | head -1); dp=DUCKDB_RDF_SHA_$DARCH; got=$(sha256sum "$f" | cut -d' ' -f1)
[ "$got" = "${!dp}" ] || die "DuckDB rdf extension hash $got, pinned ${!dp} (a new upstream build: re-pin with Jianlin)"
note "duckdb rdf extension sha256 ok ($DARCH)"

# --- 7. TripleLight at the commit under test (server, Python wheel, C library + Java classes), and the images ------------
step "repository and images"
SRC=$DATA/triplelight
[ -d "$SRC/.git" ] || git clone -q "$REPO_URL" "$SRC"
git -C "$SRC" fetch -q origin "+refs/heads/*:refs/remotes/origin/*"
if [ -n "$COMMIT" ]; then git -C "$SRC" checkout -q --detach "$COMMIT"; fi
note "triplelight $(git -C "$SRC" rev-parse HEAD)"
( cd "$SRC" && cargo +$RUST_TOOLCHAIN build -q --release --locked --features server --bin triplelight )
note "triplelight binary sha256 $(sha256sum "$SRC/target/release/triplelight" | cut -d' ' -f1)"
( cd "$SRC/bindings/python" && $V/bin/maturin build -q --release --locked -o "$T/wheels" >/dev/null )
w=$(ls -t "$T"/wheels/triplelight-*.whl | head -1); $V/bin/pip install -q --disable-pip-version-check --force-reinstall --no-deps "$w"
note "triplelight wheel $(basename "$w") sha256 $(sha256sum "$w" | cut -d' ' -f1)"
( cd "$SRC/bindings/c" && cargo +$RUST_TOOLCHAIN build -q --release --locked )
( cd "$SRC/bindings/java" && mvn -q -B -DskipTests package >/dev/null )
J=$T/triplelight-java; mkdir -p "$J"   # the FFM library and the classes stay together, from the same commit
cp "$SRC/bindings/c/target/release/libtriplelight_c.so" "$SRC"/bindings/java/target/triplelight-*.jar "$J/"
( cd "$J" && sha256sum libtriplelight_c.so triplelight-*.jar | while read -r h n; do note "java artifact $n sha256 $h"; done )
. "$SRC/bench/eval/VERSIONS"
if [ $IMAGES = 1 ]; then
  for img in "$QLEVER_IMAGE" "$VIRTUOSO_IMAGE" "$RDF4J_IMAGE"; do docker pull -q "$img" >/dev/null; note "image $img"; done
  docker image inspect "mdb:$MILLENNIUMDB_COMMIT" >/dev/null 2>&1 || ( cd "$SRC/bench/eval" && ./build-millenniumdb.sh >/dev/null )
  note "image mdb:$MILLENNIUMDB_COMMIT $(docker image inspect --format '{{.Id}}' "mdb:$MILLENNIUMDB_COMMIT")"
else note "images skipped (--skip-images)"; fi

cat > "$DATA/eval.env" <<EOF
# written by provision.sh; source before any run
export RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo JAVA_HOME=/opt/jdk25
export PATH=/opt/eval-venv/bin:/opt/jdk25/bin:/opt/cargo/bin:\$PATH
export DATA=$DATA TOOLS=$T SRC=$SRC EVAL_VENV=$V
export TRIPLELIGHT_LIBRARY=$J/libtriplelight_c.so BLAZEGRAPH_JAR=$T/blazegraph/blazegraph-jar-2.1.5.jar
export DUCKDB_EXTENSION_DIRECTORY=$EXT DUCKDB_TEMP_DIRECTORY=$DATA/scratch/duckdb
export TRIPLELIGHT_SCRATCH_DIR=$DATA/scratch/triplelight TMPDIR=$DATA/scratch
EOF
note "env file $DATA/eval.env"

# --- 8. day-one checks --------------------------------------------------------------------------------------------------------
step "checks"
# (a) a native systemd scope with a memory limit and no swap, and its cgroup files (lib.sh start_system uses the same)
systemctl reset-failed provision-probe.scope >/dev/null 2>&1 || true
systemd-run --scope --quiet --unit=provision-probe -p MemoryMax=256M -p MemorySwapMax=0 -p AllowedCPUs=0 -- sleep 3 &
sleep 1; P=/sys/fs/cgroup/system.slice/provision-probe.scope
if [ -f "$P/memory.max" ]; then note "native scope ok: memory.max $(cat "$P/memory.max") swap.max $(cat "$P/memory.swap.max") cpuset $(cat "$P/cpuset.cpus" 2>/dev/null)"
else die "systemd-run scope has no cgroup at $P" 5; fi
wait
# (b) a Docker container's own cgroup on the host, with the same limits systems.sh passes
docker image inspect debian:trixie-slim >/dev/null 2>&1 || docker pull -q debian:trixie-slim >/dev/null
cid=$(docker run -d --rm --memory=256m --memory-swap=256m --cpuset-cpus=0 debian:trixie-slim sleep 3)
C=/sys/fs/cgroup/system.slice/docker-$cid.scope
if [ -f "$C/memory.max" ]; then note "docker cgroup ok: memory.max $(cat "$C/memory.max") swap.max $(cat "$C/memory.swap.max") cpuset $(cat "$C/cpuset.cpus")"
else die "no Docker cgroup at $C" 5; fi
# (c) page-cache drop works (S2 cold runs)
sync; if echo 3 > /proc/sys/vm/drop_caches; then note "drop_caches ok"; else die "drop_caches failed" 5; fi
# (d) nothing of ours may listen on a public address: list every listener now, before any server starts
note "listeners before the run: $(ss -Hltn | awk '{print $4}' | tr '\n' ' ')"
note "free on $DATA: $(df -B1G --output=avail "$DATA" | tail -1 | tr -d ' ') GB"
step "done (receipt $R)"
