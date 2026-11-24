#!/bin/bash
# Store-size freeze table (#105): LUBM-1000 and BSBM-100M, four and six orders, one binary (main at the given commit).
# Usage: build.sh <commit>. Caller holds the lock.
set -u
D=<evidence dir>; cd $D
REPO=<repo>
T=<datasets dir>
rm -rf /tmp/alex-freeze-wt; git -C $REPO worktree add -q --detach /tmp/alex-freeze-wt $1
( cd /tmp/alex-freeze-wt && CARGO_TARGET_DIR=/tmp/alex-freeze-target cargo build --release -q --locked --features server --bin triplelight ) || exit 1
B=/tmp/alex-freeze-target/release/triplelight
echo "commit $1 binary-sha256 $(shasum -a 256 $B | cut -d' ' -f1)" > binary.txt
: > sizes.txt
for ds in lubm1000 bsbm100; do for o in 4 6; do
  out=/tmp/alex-freeze-$ds-$o; rm -rf $out
  opt=""; [ $o = 6 ] && opt="--orders=6"
  $B build-disk $T/$ds.nt $out $opt > build-$ds-$o.log 2>&1 || { echo "build $ds $o failed"; exit 1; }
  echo "== $ds $o orders" >> sizes.txt
  tot=0; for f in $out/*; do s=$(stat -f %z $f); tot=$((tot+s)); echo "$(basename $f) $s" >> sizes.txt; done
  echo "total $tot" >> sizes.txt
  rm -rf $out
done; done
git -C $REPO worktree remove --force /tmp/alex-freeze-wt; rm -rf /tmp/alex-freeze-target
echo ALL-DONE
