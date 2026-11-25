#!/usr/bin/env bash
# One benchmark session on a rented AWS host: launch -> provision -> qualify -> run -> copy results -> terminate
# (task #130 phase 6; owner Alex). Runs on the operator's machine with the operator's own AWS profile; no credential,
# token or profile is ever copied to the instance. The instance gets only a git bundle of the commit under test and
# the commands below, over SSH.
#
#   lifecycle.sh --instance-type <type> --region <r> --key-name <ec2 key> --ssh-key <private key file>
#                --security-group <sg-id> --subnet <subnet-id> --price-per-hour <USD> --max-hours <h> --max-usd <USD>
#                --commit <sha> --out <local dir> --run '<command run in the repository on the host>'
#                [--datasets 'lubm1000 bsbm100m'] [--device /dev/nvme1n1] [--ami <id>] [--dry-run]
#                [--dedicated [--dedicated-fee-per-hour 2.00]]
#
# Cost cap: refuses to launch unless (price-per-hour + dedicated fee) * max-hours <= max-usd; every amount must be a
# positive decimal. Two independent stops enforce max-hours: this script terminates the instance on every exit path
# (success, failure, Ctrl-C, hangup; a second Ctrl-C cannot skip it, and an instance whose launch was interrupted is
# found by its session tag), and the instance schedules its own `shutdown -h` at launch with shutdown behaviour
# `terminate`, so it ends even if this machine loses the session. The plan itself runs under a limit 20 minutes
# shorter, so its results are copied back first.
# Isolation: a `.metal` type, or `--dedicated` (dedicated tenancy) for any other type; shared tenancy is refused.
# --dry-run: no AWS call, no SSH. Every call it would make is printed as `DRY aws ...` / `DRY ssh ...`, the cap and
#   tenancy checks run for real, and the exit status is the one a real run would get from those checks.
# The security group must allow only tcp/22 from this machine's public address (/32); the script checks that and
# refuses anything wider. It opens no ports. The subnet must assign public IPv4 addresses. SSH runs with -F /dev/null
# and no agent forwarding, so the operator's ssh config cannot pass anything to the host. --out must be new or empty.
# Results land in <out>/: the host's receipts (provision, datasets, machine check), the run's output directory
# (bench-out/ on the host), lifecycle.log, and instance.txt (id, type, AMI, launch and terminate times, cost bound).
set -euo pipefail
TYPE=; REGION=; KEY=; SSH_KEY=; SG=; SUBNET=; PRICE=; MAX_H=; MAX_USD=; COMMIT=; OUT=; RUN=; DATASETS=; DEVICE=
AMI=; DEDICATED=0; DEDICATED_FEE=2.00; DRY=0
while [ $# -gt 0 ]; do case $1 in
  --instance-type) TYPE=$2; shift 2;; --region) REGION=$2; shift 2;; --key-name) KEY=$2; shift 2;;
  --ssh-key) SSH_KEY=$2; shift 2;; --security-group) SG=$2; shift 2;; --subnet) SUBNET=$2; shift 2;;
  --price-per-hour) PRICE=$2; shift 2;; --max-hours) MAX_H=$2; shift 2;; --max-usd) MAX_USD=$2; shift 2;;
  --commit) COMMIT=$2; shift 2;; --out) OUT=$2; shift 2;; --run) RUN=$2; shift 2;; --datasets) DATASETS=$2; shift 2;;
  --device) DEVICE=$2; shift 2;; --ami) AMI=$2; shift 2;; --dedicated) DEDICATED=1; shift;; --dry-run) DRY=1; shift;;
  --dedicated-fee-per-hour) DEDICATED_FEE=$2; shift 2;;
  *) echo "unknown option $1" >&2; exit 2;; esac; done
for v in TYPE REGION KEY SSH_KEY SG SUBNET PRICE MAX_H MAX_USD COMMIT OUT RUN; do
  [ -n "${!v}" ] || { echo "lifecycle: missing --$(echo "$v" | tr 'A-Z_' 'a-z-')" >&2; exit 2; }
done
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)

# --- checks before anything is rented ----------------------------------------------------------------------------------
num() { [[ $2 =~ ^[0-9]+(\.[0-9]+)?$ ]] && awk -v v="$2" 'BEGIN { exit !(v > 0) }' \
  || { echo "lifecycle: $1 must be a positive decimal, got '$2'" >&2; exit 2; }; }
num --price-per-hour "$PRICE"; num --max-hours "$MAX_H"; num --max-usd "$MAX_USD"; num --dedicated-fee-per-hour "$DEDICATED_FEE"
case $TYPE in *.metal*) TENANCY=default; FEE=0;; *) [ $DEDICATED = 1 ] && { TENANCY=dedicated; FEE=$DEDICATED_FEE; } \
  || { echo "lifecycle: $TYPE is not bare metal; pass --dedicated for dedicated tenancy (shared hosts are refused)" >&2; exit 4; };; esac
# a dedicated-tenancy instance is still a VM: the machine check records its steal time instead of refusing it
VM_ARG=; [ "$TENANCY" = dedicated ] && VM_ARG=--allow-vm
BOUND=$(awk -v p="$PRICE" -v f="$FEE" -v h="$MAX_H" 'BEGIN { printf "%.2f", (p + f) * h }')
awk -v b="$BOUND" -v m="$MAX_USD" 'BEGIN { exit !(b <= m) }' \
  || { echo "lifecycle: cost bound \$$BOUND ((price-per-hour + dedicated fee) x max-hours) exceeds --max-usd $MAX_USD; not launching" >&2; exit 4; }
MAX_MIN=$(awk -v h="$MAX_H" 'BEGIN { printf "%d", h * 60 }')
[ "$MAX_MIN" -ge 60 ] || { echo "lifecycle: --max-hours below 1 leaves no time to provision and copy back" >&2; exit 4; }
RUN_MIN=$((MAX_MIN - 20))   # the plan's own limit, so results are copied back before the instance stops itself
git -C "$ROOT" cat-file -e "$COMMIT^{commit}" 2>/dev/null || { echo "lifecycle: commit $COMMIT is not in $ROOT" >&2; exit 2; }
COMMIT=$(git -C "$ROOT" rev-parse "$COMMIT")
# a re-run must not overwrite the record of an earlier (possibly crashed) session
[ ! -e "$OUT" ] || [ -z "$(ls -A "$OUT")" ] || { echo "lifecycle: $OUT is not empty; use a new --out" >&2; exit 2; }
mkdir -p "$OUT"; LOG=$OUT/lifecycle.log; : > "$LOG"
exec 3>&2   # the terminal: dry-run lines go here even from steps whose output is sent to the log
log() { echo "$(date -u +%FT%TZ) $*" | tee -a "$LOG" >&2; }
info() { echo "$1 $2" >> "$OUT/instance.txt"; }
SESSION=$(uuidgen 2>/dev/null | tr 'A-Z' 'a-z' || python3 -c 'import uuid; print(uuid.uuid4())')
info session "$SESSION"; info type "$TYPE"; info tenancy "$TENANCY"; info region "$REGION"; info commit "$COMMIT"
info price_per_hour "$PRICE"; info dedicated_fee_per_hour "$FEE"; info max_hours "$MAX_H"; info cost_bound_usd "$BOUND"
info dry_run "$DRY"
log "session $SESSION: cost bound \$$BOUND over ${MAX_H}h, tenancy $TENANCY"

# --- AWS and SSH wrappers: the only places that touch the outside -------------------------------------------------------
aws_() {  # aws_ <args...>: the operator's own profile (AWS_PROFILE etc. from the environment), never passed on
  if [ $DRY = 1 ]; then echo "DRY aws --region $REGION $*" | tee -a "$LOG" >&3; return 0; fi
  aws --region "$REGION" "$@"
}
with_timeout() {  # with_timeout <seconds> <cmd...>: macOS has no timeout(1); an alarm survives exec
  perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$@"
}
HOST=
# -F /dev/null: the operator's ~/.ssh/config cannot add agent forwarding, SendEnv or other identities
SSH_OPTS=(-F /dev/null -i "$SSH_KEY" -o IdentitiesOnly=yes -o ForwardAgent=no -o ClearAllForwardings=yes -o BatchMode=yes
          -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile="$OUT/known_hosts" -o ServerAliveInterval=30
          -o ConnectTimeout=10)
ssh_() {  # ssh_ <command>: runs as admin with sudo available (Debian AMIs); the command is one shell string
  if [ $DRY = 1 ]; then echo "DRY ssh admin@<host> $*" | tee -a "$LOG" >&3; return 0; fi
  ssh "${SSH_OPTS[@]}" "admin@$HOST" "$@"
}
remote() {  # remote <script>: run a script as root on the host after sourcing eval.env; quoted once, here
  ssh_ "sudo bash -c $(printf %q ". /data/eval.env && $1")"
}
scp_to() { if [ $DRY = 1 ]; then echo "DRY scp $1 -> <host>:$2" | tee -a "$LOG" >&3; return 0; fi; scp -q "${SSH_OPTS[@]}" "$1" "admin@$HOST:$2"; }
fetch_back() {  # fetch_back <remote dir> <local dir>: copy one directory back within 10 minutes, tolerating its absence
  if [ $DRY = 1 ]; then echo "DRY copy <host>:$1 -> $2" | tee -a "$LOG" >&3; return 0; fi
  mkdir -p "$2"
  with_timeout 600 ssh "${SSH_OPTS[@]}" "admin@$HOST" "sudo tar -C $(printf %q "$(dirname "$1")") -cf - $(printf %q "$(basename "$1")") 2>/dev/null" \
    | tar -C "$2" -xf - || log "could not copy $1"
}

# --- terminate on every exit path ---------------------------------------------------------------------------------------
IID=; LAUNCHING=0
finish() {
  local rc=$?
  trap '' INT TERM HUP   # a second Ctrl-C must not skip the terminate below
  if [ -z "$IID" ] && [ $LAUNCHING = 1 ] && [ $DRY = 0 ]; then  # interrupted while run-instances was in flight
    IID=$(aws_ ec2 describe-instances --filters "Name=tag:session,Values=$SESSION" \
      "Name=instance-state-name,Values=pending,running,stopping,stopped" --query 'Reservations[].Instances[].InstanceId' --output text || true)
    [ -n "$IID" ] && log "found $IID by session tag after an interrupted launch"
  fi
  if [ -n "$IID" ]; then
    if [ -n "$HOST" ]; then fetch_back /data/receipts "$OUT" || true; fetch_back /data/bench-out "$OUT" || true; fi
    log "terminating $IID (exit $rc)"
    # shellcheck disable=SC2086  # IID may list several ids after a lookup
    aws_ ec2 terminate-instances --instance-ids $IID >/dev/null || log "TERMINATE FAILED for $IID: terminate it by hand"
    # shellcheck disable=SC2086
    [ $DRY = 1 ] || aws_ ec2 wait instance-terminated --instance-ids $IID || log "wait for termination failed for $IID: check it by hand"
    info terminated "$(date -u +%FT%TZ)"
  fi
  log "done, exit $rc"
}
trap finish EXIT
trap 'exit 130' INT TERM HUP

# --- the security group admits SSH from this machine only ----------------------------------------------------------------
if [ $DRY = 1 ]; then aws_ ec2 describe-security-groups --group-ids "$SG"; log "dry run: security group not checked"
else
  ME=$(curl -fsS --max-time 10 https://checkip.amazonaws.com | tr -d '[:space:]')
  aws_ ec2 describe-security-groups --group-ids "$SG" --query 'SecurityGroups[0].IpPermissions' --output json \
    | python3 -c 'import json, sys
me, bad = sys.argv[1] + "/32", []
for p in json.load(sys.stdin):
    proto, lo, hi = p.get("IpProtocol"), p.get("FromPort"), p.get("ToPort")
    if (proto, lo, hi) != ("tcp", 22, 22): bad.append("rule %s %s-%s" % (proto, lo, hi))
    bad += ["source " + r.get("CidrIp", "?") for r in p.get("IpRanges", []) if r.get("CidrIp") != me]
    bad += ["source " + r.get("CidrIpv6", "?") for r in p.get("Ipv6Ranges", [])]
    bad += ["a security-group or prefix-list source" for _ in p.get("UserIdGroupPairs", []) + p.get("PrefixListIds", [])]
if bad: sys.exit("lifecycle: the security group must allow only tcp/22 from " + me + "; found " + ", ".join(bad))' "$ME" \
    || exit 4
  log "security group $SG admits only tcp/22 from $ME"
fi

# --- launch -------------------------------------------------------------------------------------------------------------
if [ -z "$AMI" ]; then  # the current Debian 13 (trixie) image published by Debian, matching the instance's architecture
  ARCH=$(aws_ ec2 describe-instance-types --instance-types "$TYPE" --query 'InstanceTypes[0].ProcessorInfo.SupportedArchitectures[0]' --output text)
  [ $DRY = 1 ] && ARCH=x86_64
  case $ARCH in x86_64) A=amd64;; arm64) A=arm64;; *) echo "lifecycle: unknown architecture $ARCH" >&2; exit 4;; esac
  AMI=$(aws_ ssm get-parameter --name "/aws/service/debian/release/13/latest/$A" --query Parameter.Value --output text)
  [ $DRY = 1 ] && AMI=ami-dryrun
fi
info ami "$AMI"
# The instance's own stop: power off after max-hours; shutdown behaviour `terminate` turns that into termination.
USER_DATA="#!/bin/sh
shutdown -h +$MAX_MIN 'benchmark max-hours reached'"
LAUNCHING=1
# --client-token makes a retried call idempotent (no second instance); the session tag lets finish() find an instance
# whose launch was interrupted before its id came back
IID=$(aws_ ec2 run-instances --client-token "$SESSION" --image-id "$AMI" --instance-type "$TYPE" --key-name "$KEY" \
  --security-group-ids "$SG" --subnet-id "$SUBNET" --placement "Tenancy=$TENANCY" \
  --instance-initiated-shutdown-behavior terminate --user-data "$USER_DATA" \
  --block-device-mappings 'DeviceName=/dev/xvda,Ebs={VolumeSize=100,VolumeType=gp3,DeleteOnTermination=true}' \
  --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=triplelight-bench},{Key=session,Value=$SESSION},{Key=commit,Value=$COMMIT}]" \
  --query 'Instances[0].InstanceId' --output text)
[ $DRY = 1 ] && IID=i-dryrun
info instance_id "$IID"; info launched "$(date -u +%FT%TZ)"; log "launched $IID ($TYPE, $AMI)"
aws_ ec2 wait instance-running --instance-ids "$IID"
HOST=$(aws_ ec2 describe-instances --instance-ids "$IID" --query 'Reservations[0].Instances[0].PublicIpAddress' --output text)
[ $DRY = 1 ] && HOST=dry.invalid
case $HOST in ""|None) HOST=; log "instance has no public IPv4 address (the subnet must assign one)"; exit 5;; esac
if [ $DRY = 0 ]; then  # .metal instances take 10-15 minutes to boot
  for _ in $(seq 1 120); do ssh_ true 2>/dev/null && break; sleep 10; done
  ssh_ true || { log "no SSH to $HOST after 20 minutes"; exit 5; }
fi
log "host $HOST up"

# --- provision, datasets, qualification, run ----------------------------------------------------------------------------
BUNDLE=$OUT/triplelight-$COMMIT.bundle
if [ $DRY = 1 ]; then echo "DRY git bundle create $BUNDLE $COMMIT" | tee -a "$LOG" >&3
else  # a bundle needs a ref: a temporary branch at the commit, removed straight after
  REF=bench-bundle-$COMMIT; git -C "$ROOT" branch -f "$REF" "$COMMIT"
  git -C "$ROOT" bundle create "$BUNDLE" "$REF" 2>>"$LOG"; git -C "$ROOT" branch -D "$REF" >/dev/null
fi
scp_to "$BUNDLE" /tmp/triplelight.bundle
scp_to "$ROOT/bench/eval/provision.sh" /tmp/provision.sh
DEV_ARG=; [ -z "$DEVICE" ] || DEV_ARG="--device $(printf %q "$DEVICE")"
ssh_ "sudo bash /tmp/provision.sh $DEV_ARG --repo /tmp/triplelight.bundle --commit $COMMIT" >> "$LOG" 2>&1
if [ -n "$DATASETS" ]; then
  # shellcheck disable=SC2086  # one word per dataset name
  remote "\$SRC/bench/eval/datasets.sh $(printf '%q ' $DATASETS)" >> "$LOG" 2>&1
fi
# Qualification gates the run: a failed check stops here, and the receipts still come back.
remote "\$SRC/bench/eval/machine-check.sh /data/receipts/machine-check $VM_ARG" >> "$LOG" 2>&1 \
  || { log "machine check failed: no run (receipt in $OUT/receipts/machine-check)"; exit 6; }
if [ $DRY = 1 ]; then log "dry run: the remote steps above were printed, not executed; the run would be: $RUN"
else log "machine check passed; running (limit ${RUN_MIN} min): $RUN"; fi
remote "mkdir -p /data/bench-out && cd \"\$SRC\" && BENCH_OUT=/data/bench-out timeout ${RUN_MIN}m bash -c $(printf %q "$RUN")" >> "$LOG" 2>&1 \
  || { log "run failed or hit its time limit (results and logs still copied back)"; exit 7; }
[ $DRY = 1 ] || log "run finished"
