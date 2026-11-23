#!/bin/bash
# sha2 0.10 (main) vs 0.11 scan_bench ABBA on a four-order LUBM-1000 store (#105). Caller holds the lock.
D=<evidence dir>; cd $D
for r in 1:main 2:sha011 3:sha011 4:main; do n=${r%%:*}; b=${r#*:}
  s=$(date +%s); ./bench-$b <LUBM-1000 four-order store> --keys 5000 --passes 1 > r$n-$b.jsonl 2>/dev/null; echo "r$n $b $(( $(date +%s)-s )) s" >> progress.log
done; echo ALL-DONE >> progress.log
