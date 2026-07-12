#!/usr/bin/env bash
set -euo pipefail

# services.sh — Manage backend server and web dev client
# Usage:
#   ./scripts/services.sh start          # Start both services in background
#   ./scripts/services.sh stop           # Stop both services
#   ./scripts/services.sh restart        # Restart both services
#   ./scripts/services.sh restart-server # Restart backend only
#   ./scripts/services.sh restart-web    # Restart web dev only
#   ./scripts/services.sh status         # Show status (suggests restart if needed)
#   ./scripts/services.sh logs           # Tail logs from both
#   ./scripts/services.sh dev            # Run both in foreground (Ctrl+C to stop)

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PIDFILE_SERVER="$REPO_DIR/.pid.server"
PIDFILE_WEB="$REPO_DIR/.pid.web"
LOGDIR="$REPO_DIR/.logs"

mkdir -p "$LOGDIR"

is_server_running() {
    [[ -f "$PIDFILE_SERVER" ]] && kill -0 "$(cat "$PIDFILE_SERVER")" 2>/dev/null
}

is_web_running() {
    [[ -f "$PIDFILE_WEB" ]] && kill -0 "$(cat "$PIDFILE_WEB")" 2>/dev/null
}

# PID-file checks only know about processes *this script* started. Something
# can be listening on the port regardless — started by hand, by an IDE, by a
# previous script invocation whose PID file went stale — and this script
# would otherwise report it as "not running" despite it working fine.
backend_reachable() {
    curl -sf --max-time 2 http://127.0.0.1:3000/health >/dev/null 2>&1
}

web_reachable() {
    curl -sf --max-time 2 http://127.0.0.1:8080 >/dev/null 2>&1
}

start_server() {
    if [[ -f "$PIDFILE_SERVER" ]] && kill -0 "$(cat "$PIDFILE_SERVER")" 2>/dev/null; then
        echo "✓ Backend server already running (PID $(cat "$PIDFILE_SERVER"))"
        return
    fi
    
    # Kill any stale server processes on port 3000
    echo "Cleaning up any stale server processes on port 3000..."
    lsof -ti :3000 2>/dev/null | xargs -r kill -9 2>/dev/null || true
    sleep 1
    
    echo "Starting backend server..."
    cd "$REPO_DIR"
    nohup cargo run -p server-game >"$LOGDIR/server.log" 2>&1 &
    local server_pid=$!
    echo $server_pid > "$PIDFILE_SERVER"
    
    # Wait a moment and verify the process is still running
    sleep 2
    if kill -0 $server_pid 2>/dev/null; then
        echo "✓ Backend server started (PID $server_pid)"
    else
        echo "✗ Backend server failed to start. Check log: $LOGDIR/server.log"
        tail -20 "$LOGDIR/server.log"
        rm -f "$PIDFILE_SERVER"
        return 1
    fi
}

start_web() {
    if [[ -f "$PIDFILE_WEB" ]] && kill -0 "$(cat "$PIDFILE_WEB")" 2>/dev/null; then
        echo "✓ Web dev server already running (PID $(cat "$PIDFILE_WEB"))"
        return
    fi

    # Same defensive cleanup start_server already does — without it, an
    # untracked process already bound to :8080 (started by hand, or from a
    # stale PID file) makes dx serve fail to bind rather than taking over.
    echo "Cleaning up any stale web dev processes on port 8080..."
    lsof -ti :8080 2>/dev/null | xargs -r kill -9 2>/dev/null || true
    sleep 1

    echo "Starting web dev server..."
    cd "$REPO_DIR/crates/ui"
    nohup env RUSTC_WRAPPER="" CARGO_INCREMENTAL=0 ~/.cargo/bin/dx serve --platform web --port 8080 >"$LOGDIR/web.log" 2>&1 &
    echo $! > "$PIDFILE_WEB"
    echo "✓ Web dev server started (PID $!)"
    sleep 3
}

stop_server() {
    if [[ -f "$PIDFILE_SERVER" ]]; then
        local pid=$(cat "$PIDFILE_SERVER")
        if kill -0 "$pid" 2>/dev/null; then
            echo "Stopping backend server (PID $pid)..."
            kill "$pid" 2>/dev/null || true
            sleep 1
            if kill -0 "$pid" 2>/dev/null; then
                kill -9 "$pid" 2>/dev/null || true
            fi
            echo "✓ Backend server stopped"
        fi
        rm -f "$PIDFILE_SERVER"
    else
        echo "Backend server not running"
    fi
}

stop_web() {
    if [[ -f "$PIDFILE_WEB" ]]; then
        local pid=$(cat "$PIDFILE_WEB")
        if kill -0 "$pid" 2>/dev/null; then
            echo "Stopping web dev server (PID $pid)..."
            kill "$pid" 2>/dev/null || true
            sleep 1
            if kill -0 "$pid" 2>/dev/null; then
                kill -9 "$pid" 2>/dev/null || true
            fi
            echo "✓ Web dev server stopped"
        fi
        rm -f "$PIDFILE_WEB"
    else
        echo "Web dev server not running"
    fi
}

status() {
    echo "=== Scrabble PX Services Status ==="
    echo ""

    local server_tracked=false web_tracked=false
    is_server_running && server_tracked=true
    is_web_running && web_tracked=true
    local server_up=false web_up=false
    backend_reachable && server_up=true
    web_reachable && web_up=true

    if $server_tracked; then
        echo "✓ Backend server: running (PID $(cat "$PIDFILE_SERVER"))"
    elif $server_up; then
        echo "⚠ Backend server: responding on :3000, but not a process this script started"
        echo "  (stale/missing PID file — 'stop'/'restart-server' won't manage it as-is)"
    else
        echo "✗ Backend server: not running"
    fi
    echo "  Endpoint: http://127.0.0.1:3000"
    echo "  Log: $LOGDIR/server.log"
    echo ""

    if $web_tracked; then
        echo "✓ Web dev server: running (PID $(cat "$PIDFILE_WEB"))"
    elif $web_up; then
        echo "⚠ Web dev server: responding on :8080, but not a process this script started"
        echo "  (stale/missing PID file — 'stop'/'restart-web' won't manage it as-is)"
    else
        echo "✗ Web dev server: not running"
    fi
    echo "  URL: http://127.0.0.1:8080"
    echo "  Log: $LOGDIR/web.log"
    echo ""

    # Suggest restart if one service is down but the other is up
    if $server_tracked && ! $web_tracked && ! $web_up; then
        echo "💡 Suggestion: Restart web dev server without stopping backend"
        echo "   Run: $0 restart-web"
    elif $web_tracked && ! $server_tracked && ! $server_up; then
        echo "💡 Suggestion: Restart backend server without stopping web"
        echo "   Run: $0 restart-server"
    fi
    if $server_up && ! $server_tracked; then
        echo "💡 Suggestion: bring the backend under this script's management"
        echo "   Run: $0 restart-server   # kills whatever's on :3000, starts a tracked one"
    fi
    if $web_up && ! $web_tracked; then
        echo "💡 Suggestion: bring the web dev server under this script's management"
        echo "   Run: $0 restart-web"
    fi
}

logs() {
    echo "Tailing logs (Ctrl+C to exit)..."
    echo ""
    tail -f "$LOGDIR/server.log" "$LOGDIR/web.log" 2>/dev/null || echo "No logs yet"
}

dev() {
    echo "Starting services in foreground mode (Ctrl+C to stop)..."
    echo ""
    
    # Start backend in background
    cd "$REPO_DIR"
    cargo run -p server-game &
    SERVER_PID=$!
    sleep 2
    
    # Start web in background
    cd "$REPO_DIR/crates/ui"
    env RUSTC_WRAPPER="" CARGO_INCREMENTAL=0 ~/.cargo/bin/dx serve --platform web --port 8080 &
    WEB_PID=$!
    sleep 3
    
    echo "✓ Both services running in foreground"
    echo "  Backend: http://127.0.0.1:3000"
    echo "  Web UI: http://127.0.0.1:8080"
    echo ""
    
    # Wait for both, kill both on Ctrl+C
    trap "kill $SERVER_PID $WEB_PID 2>/dev/null; exit 0" INT TERM
    wait
}

case "${1:-help}" in
    start)
        start_server
        start_web
        status
        echo "✓ All services started"
        ;;
    stop)
        stop_web
        stop_server
        echo "✓ All services stopped"
        ;;
    status)
        status
        ;;
    logs)
        logs
        ;;
    dev)
        dev
        ;;
    restart)
        stop_web
        stop_server
        sleep 1
        start_server
        start_web
        status
        echo "✓ All services restarted"
        ;;
    restart-server)
        stop_server
        sleep 1
        start_server
        echo "✓ Backend server restarted"
        status
        ;;
    restart-web)
        stop_web
        sleep 1
        start_web
        echo "✓ Web dev server restarted"
        status
        ;;
    *)
        cat << EOF
Usage: $0 {start|stop|restart|restart-server|restart-web|status|logs|dev}

Commands:
  start          Start backend server and web dev client in background
  stop           Stop both services
  restart        Stop and restart both services
  restart-server Restart backend server only (keeps web running)
  restart-web    Restart web dev server only (keeps backend running)
  status         Show status of both services (suggests restart if needed)
  logs           Tail logs from both services
  dev            Run both services in foreground (for development)

Environment:
  Services run with:
    - Backend: cargo run -p server-game
    - Web: RUSTC_WRAPPER="" dx serve --platform web --port 8080
  
  Logs saved to: $LOGDIR/
  PIDs saved to: $REPO_DIR/.pid.*

Examples:
  $0 start
  $0 status
  $0 restart-web          # Restart web without affecting desktop clients
  $0 restart-server       # Restart server without affecting web UI
  $0 logs
  $0 dev
EOF
        exit 1
        ;;
esac
