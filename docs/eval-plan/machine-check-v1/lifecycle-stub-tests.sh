S=$1
base=(--instance-type c7i.metal-24xl --region us-east-1 --key-name k --ssh-key /dev/null --security-group sg-1 --subnet subnet-1 --commit origin/main --run true --max-hours 2 --max-usd 30)
t() { name=$1; shift; export STUB_LOG=$S/$name.calls; : > "$STUB_LOG"; PATH="$S/bin:$PATH" bench/aws/lifecycle.sh "${base[@]}" --out "$S/$name" "$@" >/dev/null 2>"$S/$name.err"; rc=$?; echo "== $name exit $rc: $(grep -c terminate-instances "$STUB_LOG") terminate call(s); $(grep -v '^DRY' "$S/$name.err" | tail -1 | cut -c1-160)"; }
t ok --price-per-hour 4.28
t badprice --price-per-hour abc
t zeroprice --price-per-hour 0
t nonempty --price-per-hour 4.28; t nonempty --price-per-hour 4.28
STUB_SG='[{"IpProtocol":"tcp","FromPort":22,"ToPort":22,"IpRanges":[{"CidrIp":"0.0.0.0/0"}]}]' t opensg --price-per-hour 4.28
STUB_SG='[{"IpProtocol":"-1","IpRanges":[{"CidrIp":"203.0.113.7/32"}]}]' t allports --price-per-hour 4.28
STUB_HANG_LAUNCH=1 t intlaunch --price-per-hour 4.28; grep -E "filters|terminate" $S/intlaunch.calls | cut -c1-140
STUB_SIGINT_IN_FINISH=1 t intfinish --price-per-hour 4.28; echo "waited for termination: $(grep -c 'wait instance-terminated' $S/intfinish.calls)"
t dedfee --instance-type m7i.8xlarge --dedicated --price-per-hour 13
STUB_SSH_RC=1 t mcfail --price-per-hour 4.28
