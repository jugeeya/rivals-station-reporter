#!/usr/bin/env bash
# Regenerates the README screenshots from the NATIVE app (crates/station-app)
# — the successor to the Playwright capture that drove the old Vue UI.
# Each shot is: seed a scratch profile (+ optional fixture state), run the
# app with RSR_SCREENSHOT=<png>, and let it capture itself and exit.
#
#   ./scripts/screenshots/native.sh          # writes docs/screenshots/*.png
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=docs/screenshots
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$OUT"
NOW=$(date +%s)
SLUG="tournament/the-hangout-47/event/rivals-2-singles"

cargo build --release -p station-app
BIN="${CARGO_TARGET_DIR:-target}/release/rivals-station-reporter"

shot() { # profile-dir seed-file-or-'' open-or-'' out.png
  local profile="$1" seed="$2" open="$3" out="$4"
  RSR_CONFIG_DIR="$profile" \
  RSR_SEED_STATE="${seed}" \
  RSR_OPEN="${open}" \
  RSR_SCREENSHOT="$OUT/$out" \
  "$BIN"
  echo "wrote $OUT/$out"
}

# ---- onboarding (no config at all) -------------------------------------------
mkdir -p "$WORK/fresh"
shot "$WORK/fresh" "" "" onboarding.png

# ---- station: idle (no set yet) -------------------------------------------------
mkdir -p "$WORK/station"
cat > "$WORK/station/config.json" <<EOF
{"mode":"station","station":3,"broker":"http://192.168.1.42:8787","slug":"$SLUG",
 "key":"k","startgg_token":"","save":"","replays":"","dir":"","idle":420,"poll":2,
 "hub_port":28787,"dry_run":false,"configured":true}
EOF
shot "$WORK/station" "" "" station-idle.png

# ---- station: live set + history ----------------------------------------------
cat > "$WORK/station-seed.json" <<EOF
{"snapshot":{
  "live":{"startEpoch":$((NOW-700)),"complete":false,"mode":"LOCAL","games":3,
    "players":[{"tag":"JUGZ!","char":"Galvan","wins":2,"slot":0,"won":true,"sgg":"jugeeya"},
               {"tag":"KIM","char":"Zetterburn","wins":1,"slot":1,"won":false}]},
  "history":[
    {"startEpoch":$((NOW-4500)),"complete":true,"mode":"LOCAL","games":4,
     "players":[{"tag":"BRUJITA","char":"Maypul","wins":3,"slot":0,"won":true,"sgg":"brujita"},
                {"tag":"JUGZ!","char":"Clairen","wins":1,"slot":1,"won":false,"sgg":"jugeeya"}]},
    {"startEpoch":$((NOW-2900)),"complete":true,"mode":"ONLINE","games":3,
     "players":[{"tag":"LOOM","char":"Ranno","wins":3,"slot":0,"won":true},
                {"tag":"KIM","char":"Kragg","wins":0,"slot":1,"won":false}]}
  ]}}
EOF
shot "$WORK/station" "$WORK/station-seed.json" "" station-live.png

# ---- operator console -----------------------------------------------------------
mkdir -p "$WORK/operator"
cat > "$WORK/operator/config.json" <<EOF
{"mode":"operator","station":1,"broker":"","slug":"$SLUG","key":"k","startgg_token":"",
 "save":"","replays":"","dir":"","idle":420,"poll":2,"hub_port":28788,"dry_run":false,
 "configured":true}
EOF
python3 scripts/screenshots/seed-hub-state.py "$WORK/operator" "$SLUG" "$NOW"
# An empty-but-present seed freezes background refreshes without touching the
# hub-state-driven console.
echo '{}' > "$WORK/empty-seed.json"
shot "$WORK/operator" "$WORK/empty-seed.json" "" operator-console.png

# ---- Current Sets panel (seeded start.gg data; scrolled into view) ---------------
RSR_SCROLL=end shot "$WORK/operator" "$WORK/operator/available-seed.json" "" available-sets.png

# ---- station: set just finished ---------------------------------------------------
cat > "$WORK/finished-seed.json" <<EOF
{"snapshot":{
  "live": null,
  "history":[
    {"startEpoch":$((NOW-1320)),"complete":true,"mode":null,"games":4,
     "players":[{"tag":"LOOM","char":"Zetterburn","wins":3,"slot":0,"won":true},
                {"tag":"KIM","char":"Kragg","wins":1,"slot":1,"won":false}]}
  ]}}
EOF
shot "$WORK/station" "$WORK/finished-seed.json" "" station-finished.png

# ---- settings drawer ------------------------------------------------------------
shot "$WORK/station" "$WORK/station-seed.json" settings settings.png

echo "done — $(ls "$OUT" | wc -l | tr -d ' ') screenshots in $OUT"
