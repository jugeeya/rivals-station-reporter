#!/usr/bin/env bash
# Regenerates the README screenshots from the native app (crates/station-app).
# Each shot is: seed a scratch profile (+ fixture state), run the app with
# RSR_SCREENSHOT=<png>, and let it capture itself and exit.
#
# One coherent story across every shot (see seed-hub-state.py for the full
# cast): the operator watches stations 1-3; station-live.png is station 2's
# own view of the same BRUJITA/NAVI set the console shows; station-finished
# is station 3's view of the LOOM/SLADE set awaiting report; station-idle is
# a fourth station between sets.
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

# Every station shot shares this healthy-station health block and a pinned
# status line, so captures never show the CAPTURING machine's paths.
HEALTH='{"savePath":"C:\\Users\\Station\\AppData\\Local\\Rivals2\\Saved\\SaveGames\\Rivals2_StatsSaveSlot.sav","saveExists":true,"saveArmed":true,"replaysPath":"C:\\Users\\Station\\AppData\\Local\\Rivals2\\Saved\\Replays","replaysExists":true,"outDir":"C:\\Users\\Station\\AppData\\Roaming\\rivals-station-reporter\\out"}'

cargo build --release -p station-app
BIN="${CARGO_TARGET_DIR:-target}/release/rivals-station-reporter"

station_config() { # dir station-number
  mkdir -p "$1"
  cat > "$1/config.json" <<EOF
{"mode":"station","station":$2,"broker":"http://192.168.1.42:8787","slug":"$SLUG",
 "key":"k","startgg_token":"","save":"","replays":"","dir":"","idle":420,"poll":2,
 "hub_port":28787,"dry_run":false,"configured":true}
EOF
}

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

# ---- station 4: idle between sets ----------------------------------------------
station_config "$WORK/st-idle" 4
cat > "$WORK/idle-seed.json" <<EOF
{"health":$HEALTH,
 "status":{"msg":"stats source armed | tags: KAZE, PIP","error":false,"t":$NOW},
 "snapshot":{
  "live": null,
  "history":[
    {"startEpoch":$((NOW-5400)),"complete":true,"mode":null,"games":4,
     "players":[{"tag":"KAZE","char":"Orcane","wins":3,"slot":0,"won":true,"sgg":"kaze"},
                {"tag":"PIP","char":"Etalus","wins":1,"slot":1,"won":false}]},
    {"startEpoch":$((NOW-1800)),"complete":true,"mode":"ONLINE","games":2,
     "players":[{"tag":"KAZE","char":"Absa","wins":2,"slot":0,"won":true,"sgg":"kaze"},
                {"tag":"RANDO","char":"Wrastor","wins":0,"slot":1,"won":false}]}
  ]}}
EOF
shot "$WORK/st-idle" "$WORK/idle-seed.json" "" station-idle.png

# ---- station 2: mid-set (the console's BRUJITA/NAVI set, from its own PC) -------
station_config "$WORK/st-live" 2
cat > "$WORK/live-seed.json" <<EOF
{"health":$HEALTH,
 "status":{"msg":"tracking live: BRUJITA vs NAVI","error":false,"t":$NOW},
 "snapshot":{
  "live":{"startEpoch":$((NOW-360)),"complete":false,"mode":null,"games":3,
    "players":[{"tag":"BRUJITA","char":"Maypul","wins":2,"slot":0,"won":true,"sgg":"brujita"},
               {"tag":"NAVI","char":"Fleet","wins":1,"slot":1,"won":false}]},
  "history":[]}}
EOF
shot "$WORK/st-live" "$WORK/live-seed.json" "" station-live.png

# ---- station 3: set just finished (the console's LOOM/SLADE set) ---------------
station_config "$WORK/st-fin" 3
cat > "$WORK/fin-seed.json" <<EOF
{"health":$HEALTH,
 "status":{"msg":"finalized set_$((NOW-2000)).json: LOOM wins (4 games, complete=true)","error":false,"t":$NOW},
 "snapshot":{
  "live": null,
  "history":[
    {"startEpoch":$((NOW-2000)),"complete":true,"mode":null,"games":4,
     "players":[{"tag":"LOOM","char":"Zetterburn","wins":3,"slot":0,"won":true},
                {"tag":"SLADE","char":"Kragg","wins":1,"slot":1,"won":false}]}
  ]}}
EOF
shot "$WORK/st-fin" "$WORK/fin-seed.json" "" station-finished.png

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

# ---- settings drawer ------------------------------------------------------------
shot "$WORK/st-idle" "$WORK/idle-seed.json" settings settings.png

echo "done — $(ls "$OUT" | wc -l | tr -d ' ') screenshots in $OUT"
