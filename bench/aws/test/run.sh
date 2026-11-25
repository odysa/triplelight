#!/usr/bin/env bash
# Failure-path tests for bench/aws/lifecycle.sh against stub aws/ssh/scp/curl (no AWS account, no network):
#   bench/aws/test/run.sh        exits nonzero if any case does not behave as listed.
set -u
HERE=$(cd "$(dirname "$0")" && pwd); LC=$HERE/../lifecycle.sh
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
BASE=(--instance-type c7i.metal-24xl --region us-east-1 --key-name k --ssh-key /dev/null --security-group sg-1
      --subnet subnet-1 --commit HEAD --run 'echo "it'"'"'s quoted"' --datasets 'lubm1000 bsbm100m' --max-hours 2 --max-usd 30)
FAILED=0
# case <name> <expected exit> <expected terminate calls> [lifecycle args...]; environment variables steer the stubs
case_() {
  local name=$1 want_rc=$2 want_term=$3; shift 3
  export STUB_LOG=$W/$name.calls; : > "$STUB_LOG"
  PATH="$HERE/bin:$PATH" "$LC" "${BASE[@]}" --out "$W/$name" "$@" > "$W/$name.out" 2> "$W/$name.err"; local rc=$?
  local term; term=$(grep -c terminate-instances "$STUB_LOG")
  if [ "$rc" = "$want_rc" ] && [ "$term" = "$want_term" ]; then echo "ok   $name (exit $rc, $term terminate)"
  else echo "FAIL $name: exit $rc (want $want_rc), $term terminate calls (want $want_term)"; tail -3 "$W/$name.err"; FAILED=1; fi
}
case_ full-session 0 1 --price-per-hour 4.28
grep -Eq 'timeout\\? [0-9]+m\\? bash' "$W/full-session.calls" || { echo "FAIL full-session: the run is not under timeout"; FAILED=1; }
case_ price-not-a-number 2 0 --price-per-hour abc
case_ price-zero 2 0 --price-per-hour 0
case_ over-cap 4 0 --price-per-hour 20
case_ shared-tenancy 4 0 --instance-type c7i.8xlarge --price-per-hour 1.4
case_ dedicated-fee-over-cap 4 0 --instance-type c7i.8xlarge --dedicated --price-per-hour 13.5
mkdir -p "$W/reused" && touch "$W/reused/instance.txt"; case_ reused 2 0 --price-per-hour 4.28
STUB_SG='[{"IpProtocol":"tcp","FromPort":22,"ToPort":22,"IpRanges":[{"CidrIp":"0.0.0.0/0"}]}]' case_ sg-open-to-all 4 0 --price-per-hour 4.28
STUB_SG='[{"IpProtocol":"-1","IpRanges":[{"CidrIp":"203.0.113.7/32"}]}]' case_ sg-all-ports 4 0 --price-per-hour 4.28
STUB_SG='[{"IpProtocol":"tcp","FromPort":22,"ToPort":22,"Ipv6Ranges":[{"CidrIpv6":"::/0"}]}]' case_ sg-ipv6 4 0 --price-per-hour 4.28
STUB_SIGNAL_IN_LAUNCH=INT case_ int-during-launch 130 1 --price-per-hour 4.28
STUB_SIGNAL_IN_LAUNCH=HUP STUB_TAG_MISSES=2 case_ hup-during-launch-slow-tag 130 1 --price-per-hour 4.28
[ "$(grep -c -- --filters "$W/hup-during-launch-slow-tag.calls")" = 3 ] || { echo "FAIL hup-during-launch-slow-tag: did not poll the tag"; FAILED=1; }
STUB_SIGINT_IN_FINISH=1 case_ int-during-terminate 0 1 --price-per-hour 4.28
grep -q "wait instance-terminated" "$W/int-during-terminate.calls" || { echo "FAIL int-during-terminate: did not wait for termination"; FAILED=1; }
STUB_MC_FAIL=1 case_ machine-check-fails 6 1 --price-per-hour 4.28
! grep -Eq 'timeout\\? [0-9]+m' "$W/machine-check-fails.calls" \
  || { echo "FAIL machine-check-fails: the run started"; FAILED=1; }
# a terminal that is gone: stderr closed, then an interrupted launch; terminate must still happen
export STUB_LOG=$W/closed-stderr.calls; : > "$STUB_LOG"
STUB_SIGNAL_IN_LAUNCH=HUP PATH="$HERE/bin:$PATH" "$LC" "${BASE[@]}" --out "$W/closed-stderr" --price-per-hour 4.28 >/dev/null 2>&-
[ "$(grep -c terminate-instances "$STUB_LOG")" = 1 ] && echo "ok   closed-stderr (terminate issued)" || { echo "FAIL closed-stderr: no terminate"; FAILED=1; }
exit $FAILED
