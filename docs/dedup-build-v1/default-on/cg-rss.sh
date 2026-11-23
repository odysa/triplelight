#!/bin/sh
# Inside the container: run the build, sample its VmHWM and the cgroup's anon bytes every 0.5 s.
"$@" & pid=$!; hwm=0; anon=0
while kill -0 $pid 2>/dev/null; do
  h=$(awk '/VmHWM/{print $2}' /proc/$pid/status 2>/dev/null); [ -n "$h" ] && [ "$h" -gt "$hwm" ] && hwm=$h
  a=$(awk '$1=="anon"{print $2}' /sys/fs/cgroup/memory.stat); [ "$a" -gt "$anon" ] && anon=$a
  sleep 0.5
done
wait $pid; echo "exit=$? vmhwm_kib=$hwm anon_peak_bytes=$anon"
