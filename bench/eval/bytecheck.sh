#!/usr/bin/env bash
# Day-one check before timing large answers by draining (driver.py --drain-rows, plan B8): a system's JSON body for one request must
# have the same byte count every time, or every large query would come out `unstable`. Sends one query twice to each running
# system, prints both byte counts and the HTTP status, and exits 1 if any pair differs or a request fails.
#   bytecheck.sh [-q QUERY.rq] SYSTEM...        (the systems must already be serving; URLs come from lib.sh url_of)
#   bytecheck.sh [-q QUERY.rq] -u URL           (any endpoint)
# Default query: the LUBM Q6 file next to this repository's bench/lubm. Record the output in the day-one receipt.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
Q=$HERE/../lubm/q6.rq; URLS=(); NAMES=()
. "$HERE/lib.sh"
while [ $# -gt 0 ]; do case $1 in
  -q) Q=$2; shift 2;;
  -u) NAMES+=("$2"); URLS+=("$2"); shift 2;;
  *) u=$(url_of "$1") || exit 2; NAMES+=("$1"); URLS+=("$u"); shift;;
esac; done
[ ${#URLS[@]} -gt 0 ] || { echo "usage: bytecheck.sh [-q QUERY.rq] SYSTEM... | -u URL" >&2; exit 2; }
[ -f "$Q" ] || { echo "no query file $Q" >&2; exit 2; }
bad=0
for i in "${!URLS[@]}"; do
  one() { curl -s -o /dev/null -w '%{http_code} %{size_download}' --max-time 600 -H 'Accept: application/sparql-results+json' --data-urlencode "query@$Q" "${URLS[$i]}"; }
  a=$(one); b=$(one)
  if [ "$a" = "$b" ] && [ "${a%% *}" = 200 ]; then s=ok; else s=DIFFERENT; bad=1; fi
  printf '%-18s run1 %s  run2 %s  %s\n' "${NAMES[$i]}" "$a" "$b" "$s"
done
exit $bad
