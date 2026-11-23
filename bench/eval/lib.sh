url_of() {   # the SPARQL endpoint of a system
  case $1 in triplelight) echo http://127.0.0.1:7979/sparql;; qlever) echo http://127.0.0.1:7001/;; virtuoso) echo http://127.0.0.1:8890/sparql;;
             jena-tdb2) echo http://127.0.0.1:3030/ds/sparql;; oxigraph) echo http://127.0.0.1:7878/query;;
             qlever-two-order) echo http://127.0.0.1:7001/;; rdf4j-native|rdf4j-native-default) echo http://127.0.0.1:3040/rdf4j-server/repositories/bench;;
             millenniumdb) echo http://127.0.0.1:1234/sparql;; *) echo "unknown system $1" >&2; return 1;; esac
}
port_of() { local u; u=$(url_of "$1"); u=${u#http://127.0.0.1:}; echo "${u%%/*}"; }
PIDFILE() { echo "$OUT/$1.pid"; }
stop_system() {   # stop_system <system>
  local s=$1
  case $s in qlever|virtuoso) docker rm -f "bench-$s" >/dev/null 2>&1 || true ;;
             qlever-two-order) docker rm -f bench-qlever >/dev/null 2>&1 || true ;;
             rdf4j-native|rdf4j-native-default) docker rm -f bench-rdf4j >/dev/null 2>&1 || true ;;
             millenniumdb) docker rm -f bench-mdb >/dev/null 2>&1 || true ;; esac
  if [ -f "$(PIDFILE "$s")" ]; then kill "$(cat "$(PIDFILE "$s")")" 2>/dev/null || true; wait "$(cat "$(PIDFILE "$s")")" 2>/dev/null || true; rm -f "$(PIDFILE "$s")"; fi
  # a leftover listener would make the next run measure a stale server
  local port; port=$(port_of "$s")
  for _ in $(seq 1 30); do lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1 || return 0; sleep 1; done
  echo "port $port still busy after stopping $s" >&2; return 1
}
is_docker_system() { case $1 in qlever|qlever-two-order|virtuoso|rdf4j-native|rdf4j-native-default|millenniumdb) return 0;; *) return 1;; esac; }
start_system() {  # start_system <system> <dataset> [budget args...]; waits until it answers and, for triplelight, until statistics are ready
  local s=$1 d=$2; shift 2
  local port; port=$(port_of "$s")
  if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then echo "port $port busy; refusing to measure a stale server" >&2; return 1; fi
  : > "$OUT/$s.server.log"
  # S1 (Linux): a native server runs in its own cgroup scope with the session's memory limit and no swap, so it is limited
  # and measured like the Docker systems (whose containers get --memory). Docker systems are started as before.
  # Needs root (the rental runs the harness as root) or a user manager with the memory controller delegated; if
  # systemd-run fails, the server does not start and session.sh records the failure. A memory-limited run must name its
  # limit: there is no default here, so every system gets the same memory.max.
  local scope=()
  if [ "$(uname)" = Linux ] && command -v systemd-run >/dev/null && ! is_docker_system "$s"; then
    local mem; mem=$(printf '%s\n' "$@" | awk 'p{print; exit} $0=="--mem"{p=1}')
    [ -n "$mem" ] || { echo "start_system $s: no --mem in the budget; refusing to start a native server without a memory limit" >&2; return 1; }
    echo "start_system $s: native server in scope bench-$s, MemoryMax=${mem}G, no swap" >> "$OUT/$s.server.log"
    systemctl stop "bench-$s.scope" >/dev/null 2>&1 || true
    scope=(systemd-run --scope --quiet --unit="bench-$s" -p MemoryMax="${mem}G" -p MemorySwapMax=0 --)
  fi
  ${scope[@]+"${scope[@]}"} "$HERE/systems.sh" "$s" serve "$d" "$@" > "$OUT/$s.server.log" 2>&1 &
  echo $! > "$(PIDFILE "$s")"
  local t0; t0=$(date +%s)
  # readiness probe with a constant predicate: QLever's two-order index cannot answer a variable predicate and drops the connection
  until curl -sf -o /dev/null --data-urlencode 'query=ASK{?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> ?o}' "$(url_of "$s")"; do
    sleep 2; kill -0 "$(cat "$(PIDFILE "$s")")" 2>/dev/null || { echo "$s exited while starting" >&2; return 1; }
  done
  echo "$s ready after $(( $(date +%s) - t0 ))s" >> "$OUT/$s.server.log"
  if [ "$s" = triplelight ] && grep -q "building in background" "$OUT/$s.server.log"; then
    until grep -qE "statistics (ready|failed)" "$OUT/$s.server.log"; do sleep 2; done
  fi
}
drop_caches() {   # prints what it did; the Linux machine drops the page cache, the laptop usually cannot
  if [ -w /proc/sys/vm/drop_caches ]; then sync; echo 3 > /proc/sys/vm/drop_caches && echo "page cache dropped"
  elif sudo -n purge >/dev/null 2>&1; then echo "page cache purged"
  else echo "page cache NOT dropped (no permission)"; fi
}
