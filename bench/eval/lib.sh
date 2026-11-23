url_of() {   # the SPARQL endpoint of a system
  case $1 in triplelight) echo http://127.0.0.1:7979/sparql;; qlever) echo http://127.0.0.1:7001/;; virtuoso) echo http://127.0.0.1:8890/sparql;;
             jena-tdb2) echo http://127.0.0.1:3030/ds/sparql;; oxigraph) echo http://127.0.0.1:7878/query;;
             qlever-two-order) echo http://127.0.0.1:7001/;; rdf4j-native|rdf4j-native-default) echo http://127.0.0.1:8080/rdf4j-server/repositories/bench;;
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
start_system() {  # start_system <system> <dataset> [budget args...]; waits until it answers and, for triplelight, until statistics are ready
  local s=$1 d=$2; shift 2
  local port; port=$(port_of "$s")
  if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then echo "port $port busy; refusing to measure a stale server" >&2; return 1; fi
  : > "$OUT/$s.server.log"
  "$HERE/systems.sh" "$s" serve "$d" "$@" > "$OUT/$s.server.log" 2>&1 &
  echo $! > "$(PIDFILE "$s")"
  local t0; t0=$(date +%s)
  until curl -s -o /dev/null --data-urlencode 'query=ASK{?s ?p ?o}' "$(url_of "$s")"; do
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
