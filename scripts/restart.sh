#!/bin/sh
# Called after a successful bundle build by `make restart`.
set -eu
cd "$(dirname "$0")/.."

PROJECT_DIR=$(pwd -P)
APP="$PROJECT_DIR/target/bundle/Arabic Term.app"
LOG="$PROJECT_DIR/target/restart.log"

if [ ! -x "$APP/Contents/MacOS/arabic-term" ] || [ ! -f "$APP/Contents/Info.plist" ]; then
  echo "Build the app first with make bundle." >&2
  exit 1
fi

case "${1:-}" in
  '')
    # The invoking shell may be inside the app we are about to close. Ignore
    # its hangup and detach all terminal I/O before stopping any processes.
    nohup /bin/sh "$PROJECT_DIR/scripts/restart.sh" --detached </dev/null >"$LOG" 2>&1 &
    echo "Restarting Arabic Term; its current sessions will close."
    echo "Restart log: $LOG"
    exit 0
    ;;
  --detached) ;;
  *) echo "Usage: sh scripts/restart.sh" >&2; exit 2 ;;
esac

# Recognize copies launched from the Dock or Finder by their bundle ID, plus
# direct executable builds from this checkout. Ignore unrelated test apps.
PROCESSES=$(ps -axww -o pid=,comm=)
PIDS=$(printf '%s\n' "$PROCESSES" | while read -r pid executable; do
  case "$executable" in
    ("$APP/Contents/MacOS/arabic-term"|"$PROJECT_DIR/target/debug/arabic-term"|"$PROJECT_DIR/target/release/arabic-term") ;;
    (*/Contents/MacOS/arabic-term)
      bundle=${executable%/Contents/MacOS/arabic-term}
      identifier=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$bundle/Contents/Info.plist" 2>/dev/null) || continue
      [ "$identifier" = 'dev.almodhfer.arabic-term' ] || continue
      ;;
    (*) continue ;;
  esac
  printf '%s\n' "$pid"
done)

for pid in $PIDS; do
  # A process can exit between the snapshot and the signal.
  if ! kill -TERM "$pid" 2>/dev/null && kill -0 "$pid" 2>/dev/null; then
    echo "Could not stop Arabic Term process $pid." >&2
    exit 1
  fi
done

attempt=0
while :; do
  pending=false
  for pid in $PIDS; do
    if kill -0 "$pid" 2>/dev/null; then pending=true; fi
  done
  if [ "$pending" = false ]; then break; fi
  if [ "$attempt" -ge 50 ]; then
    echo "Timed out waiting for Arabic Term to exit; no new instance was opened." >&2
    exit 1
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done

open -n "$APP"
echo "Opened $APP"
