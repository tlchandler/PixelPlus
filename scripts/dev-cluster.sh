#!/usr/bin/env bash
# dev-cluster.sh - run a leader and two followers of pixelplusd on one machine.
#
#   scripts/dev-cluster.sh start [--fresh]   start all three (build first with `cargo build`)
#   scripts/dev-cluster.sh stop              stop all three
#   scripts/dev-cluster.sh restart NODE      restart one node (leader|f1|f2)
#   scripts/dev-cluster.sh kill NODE         SIGKILL one node (simulate a crash / power cut)
#   scripts/dev-cluster.sh status            PIDs, URLs and /public/health of each node
#   scripts/dev-cluster.sh logs [NODE]       tail -f the log(s)
#   scripts/dev-cluster.sh env NODE          print the environment used for a node
#
# Every node gets its own data directory, HTTP port and UDP cluster ports and
# finds the others through PIXELPLUS_CLUSTER_PEERS on 127.0.0.1 (broadcast and
# mDNS are off so nothing leaks onto the LAN). Output is the in-memory
# simulator, audio is off and PIXELPLUS_DEV=1 enables GET /api/v1/debug/output.
#
# Environment knobs (all optional):
#   PP_CLUSTER_DIR   state/log directory      (default: ./.dev-cluster)
#   PP_BIN           pixelplusd binary        (default: target/debug/pixelplusd)
#   PP_WEB_DIR       built web UI             (default: web/build)
#   PP_HTTP_BASE     leader HTTP port; f1 = +1, f2 = +2   (default: 18080)
#   PP_CLUSTER_BASE  leader UDP port; f1 = +10, f2 = +20 (overlay = port + 1) (default: 32420)
#   PP_AUDIO         PIXELPLUS_AUDIO for the leader (default: none)
#   PP_LOG           PIXELPLUS_LOG filter     (default: info,tower_http=warn)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="${PP_CLUSTER_DIR:-$ROOT/.dev-cluster}"
BIN="${PP_BIN:-$ROOT/target/debug/pixelplusd}"
WEB="${PP_WEB_DIR:-$ROOT/web/build}"
HTTP_BASE="${PP_HTTP_BASE:-18080}"
UDP_BASE="${PP_CLUSTER_BASE:-32420}"
NODES=(leader f1 f2)

idx() {
	case "$1" in
	leader) echo 0 ;;
	f1) echo 1 ;;
	f2) echo 2 ;;
	*)
		echo "unknown node '$1' (leader|f1|f2)" >&2
		exit 2
		;;
	esac
}
http_port() { echo $((HTTP_BASE + $(idx "$1"))); }
udp_port() { echo $((UDP_BASE + 10 * $(idx "$1"))); }
board() { if [ "$1" = leader ]; then echo difftxlarge; else echo difftx; fi; }
peers() {
	local me="$1" n out=""
	for n in "${NODES[@]}"; do
		[ "$n" = "$me" ] && continue
		out="${out:+$out,}127.0.0.1:$(udp_port "$n")"
	done
	echo "$out"
}
pidfile() { echo "$DIR/$1.pid"; }
running() {
	local p
	p="$(cat "$(pidfile "$1")" 2>/dev/null || true)"
	[ -n "$p" ] && kill -0 "$p" 2>/dev/null
}

node_env() {
	local n="$1" audio=none
	[ "$n" = leader ] && audio="${PP_AUDIO:-none}"
	cat <<EOF
PIXELPLUS_DATA_DIR=$DIR/$n
PIXELPLUS_WEB_DIR=$WEB
PIXELPLUS_HTTP_PORT=$(http_port "$n")
PIXELPLUS_HTTP_BIND=127.0.0.1
PIXELPLUS_CLUSTER_PORT=$(udp_port "$n")
PIXELPLUS_CLUSTER_OVERLAY_PORT=$(($(udp_port "$n") + 1))
PIXELPLUS_CLUSTER_PEERS=$(peers "$n")
PIXELPLUS_CLUSTER_BROADCAST=0
PIXELPLUS_MDNS=0
PIXELPLUS_OUTPUT=sim
PIXELPLUS_BOARD=$(board "$n")
PIXELPLUS_AUDIO=$audio
PIXELPLUS_SHM_DIR=$DIR/$n/shm
PIXELPLUS_RUN_DIR=$DIR/$n/run
PIXELPLUS_GAMES_SOCKET=$DIR/$n/run/games.sock
PIXELPLUS_TTS_URL=http://127.0.0.1:9
PIXELPLUS_DEV=1
PIXELPLUS_LOG=${PP_LOG:-info,tower_http=warn}
EOF
}

start_one() {
	local n="$1"
	if running "$n"; then
		echo "$n already running (pid $(cat "$(pidfile "$n")"))"
		return
	fi
	mkdir -p "$DIR/$n/shm" "$DIR/$n/run"
	local envs=()
	while IFS= read -r line; do envs+=("$line"); done < <(node_env "$n")
	env "${envs[@]}" "$BIN" >>"$DIR/$n.log" 2>&1 &
	echo $! >"$(pidfile "$n")"
	echo "$n: http://127.0.0.1:$(http_port "$n")  (pid $!, log $DIR/$n.log)"
}

wait_healthy() {
	local n="$1" i
	for i in $(seq 1 100); do
		if curl -fsS "http://127.0.0.1:$(http_port "$n")/api/v1/public/health" >/dev/null 2>&1; then
			return 0
		fi
		if ! running "$n"; then
			echo "$n exited; last log lines:" >&2
			tail -n 20 "$DIR/$n.log" >&2 || true
			return 1
		fi
		sleep 0.1
		: "$i"
	done
	echo "$n did not answer within 10 s" >&2
	return 1
}

stop_one() {
	local n="$1" sig="${2:-TERM}" p i
	p="$(cat "$(pidfile "$n")" 2>/dev/null || true)"
	if [ -n "$p" ] && kill -0 "$p" 2>/dev/null; then
		kill "-$sig" "$p" 2>/dev/null || true
		for i in $(seq 1 50); do
			kill -0 "$p" 2>/dev/null || break
			sleep 0.1
			: "$i"
		done
		kill -0 "$p" 2>/dev/null && kill -KILL "$p" 2>/dev/null || true
		echo "$n stopped"
	fi
	rm -f "$(pidfile "$n")"
}

cmd="${1:-status}"
shift || true
case "$cmd" in
start)
	if [ "${1:-}" = "--fresh" ]; then
		for n in "${NODES[@]}"; do stop_one "$n"; done
		rm -rf "$DIR"
	fi
	[ -x "$BIN" ] || {
		echo "no daemon at $BIN - run: cargo build -p pixelplus-daemon" >&2
		exit 1
	}
	[ -f "$WEB/index.html" ] || echo "note: $WEB/index.html missing - run: (cd web && pnpm build)" >&2
	mkdir -p "$DIR"
	for n in "${NODES[@]}"; do start_one "$n"; done
	for n in "${NODES[@]}"; do wait_healthy "$n"; done
	;;
stop)
	for n in "${NODES[@]}"; do stop_one "$n"; done
	;;
restart)
	n="${1:?node (leader|f1|f2)}"
	idx "$n" >/dev/null
	stop_one "$n"
	start_one "$n"
	wait_healthy "$n"
	;;
kill)
	n="${1:?node (leader|f1|f2)}"
	idx "$n" >/dev/null
	stop_one "$n" KILL
	;;
status)
	for n in "${NODES[@]}"; do
		if running "$n"; then
			h="$(curl -fsS "http://127.0.0.1:$(http_port "$n")/api/v1/public/health" 2>/dev/null || echo 'not answering')"
			echo "$n  pid $(cat "$(pidfile "$n")")  http://127.0.0.1:$(http_port "$n")  udp $(udp_port "$n")  $h"
		else
			echo "$n  stopped"
		fi
	done
	;;
logs)
	if [ -n "${1:-}" ]; then
		idx "$1" >/dev/null
		tail -n 50 -f "$DIR/$1.log"
	else
		tail -n 20 -f "$DIR"/leader.log "$DIR"/f1.log "$DIR"/f2.log
	fi
	;;
env)
	node_env "${1:?node (leader|f1|f2)}"
	;;
*)
	sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
	exit 2
	;;
esac
