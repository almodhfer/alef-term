#!/bin/sh
# Called after a successful bundle build by `make restart`.
set -eu
cd "$(dirname "$0")/.."

PROJECT_DIR=$(pwd -P)
APP="$PROJECT_DIR/target/bundle/Alef.app"
LOG="$PROJECT_DIR/target/restart.log"

if [ ! -x "$APP/Contents/MacOS/alef-term" ] || [ ! -f "$APP/Contents/Info.plist" ]; then
  echo "Build the app first with make bundle." >&2
  exit 1
fi

case "${1:-}" in
  '')
    # The invoking shell may be inside the app we are about to close. Ignore
    # its hangup and detach all terminal I/O before stopping any processes.
    nohup /bin/sh "$PROJECT_DIR/scripts/restart.sh" --detached </dev/null >"$LOG" 2>&1 &
    echo "Restarting Alef; its current sessions will close."
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
    ("$APP/Contents/MacOS/alef-term"|"$PROJECT_DIR/target/debug/alef-term"|"$PROJECT_DIR/target/release/alef-term") ;;
    (*/Contents/MacOS/alef-term)
      bundle=${executable%/Contents/MacOS/alef-term}
      identifier=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$bundle/Contents/Info.plist" 2>/dev/null) || continue
      [ "$identifier" = 'dev.almodhfer.alef-term' ] || continue
      ;;
    (*) continue ;;
  esac
  printf '%s\n' "$pid"
done)

for pid in $PIDS; do
  # A process can exit between the snapshot and the signal.
  if ! kill -TERM "$pid" 2>/dev/null && kill -0 "$pid" 2>/dev/null; then
    echo "Could not stop Alef process $pid." >&2
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
    echo "Timed out waiting for Alef to exit; no new instance was opened." >&2
    exit 1
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done

open -n "$APP"
echo "Opened $APP"
