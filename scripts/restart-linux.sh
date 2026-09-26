#!/bin/sh
# Called after a successful build by `make restart` on Linux.
set -eu
cd "$(dirname "$0")/.."

PROJECT_DIR=$(pwd -P)
BIN="$PROJECT_DIR/target/release/arabic-term"
LOG="$PROJECT_DIR/target/restart.log"

case "${1:-}" in
  '')
    # The invoking shell may be inside a window we are about to close. Start
    # a new session so the restart outlives it.
    setsid nohup /bin/sh "$PROJECT_DIR/scripts/restart-linux.sh" --detached </dev/null >"$LOG" 2>&1 &
    echo "Restarting Arabic Term; its current sessions will close."
    echo "Restart log: $LOG"
    exit 0
    ;;
  --detached) ;;
  *) echo "Usage: sh scripts/restart-linux.sh" >&2; exit 2 ;;
esac

pkill -x arabic-term || true
for _ in 1 2 3 4 5 6 7 8 9 10; do
  pgrep -x arabic-term >/dev/null || break
  sleep 0.2
done
exec "$BIN"
