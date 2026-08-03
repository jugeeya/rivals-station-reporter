#!/usr/bin/env python3
"""Writes the operator fixture hub-state.json into a profile dir — a direct
port of the operatorConsole scene from the old fixtures.mjs, so the native
README screenshot shows the same three stations, the same per-game character
strips, and the same start.gg best-of/elapsed overrides the web one did.
Shapes mirror what the hub actually persists (station-core/src/hub.rs)."""
import json
import sys

profile, slug, now = sys.argv[1], sys.argv[2], int(sys.argv[3])


def to_id(epoch):
    import datetime
    d = datetime.datetime.utcfromtimestamp(epoch)
    return d.strftime("%Y%m%d_%H%M%S")


def game(num, winner, c0, c1):
    return {"gameNum": num, "winnerSlot": winner,
            "chars": [{"slot": 0, "character": c0}, {"slot": 1, "character": c1}]}


# Station 1: just bound, no games yet; start.gg overrides visible ("first to
# 4" from totalGames=7, elapsed from startggStartedAt 18m ago vs the
# station's own 12m guess).
station1 = {
    "id": to_id(now - 720), "station": 1, "ingestedAt": now - 30,
    "set": {"setId": to_id(now - 720), "complete": False, "startEpoch": now - 720,
            "endEpoch": None, "winsRequired": 3, "matchCount": 0,
            "winnerSlot": None, "winnerName": None,
            "players": [{"slot": 0, "name": "jugeeya", "character": None, "wins": 0},
                        {"slot": 1, "name": "Kimchi", "character": None, "wins": 0}],
            "games": []},
    "matchedStartggSetId": "sgg-set-201", "fullRoundText": "Winners Quarter-Final",
    "entrants": [{"id": "E1", "name": "jugeeya"}, {"id": "E2", "name": "Kimchi"}],
    "slotEntrants": [{"slot": 0, "entrantId": "E1", "entrantName": "jugeeya"},
                     {"slot": 1, "entrantId": "E2", "entrantName": "Kimchi"}],
    "candidateWinnerEntrantId": None, "confidence": "none", "status": "live",
    "liveConfirmed": False, "swap": False, "mode": None, "startggState": 2,
    "startggStartedAt": now - 1080, "startggTotalGames": 7,
    "reportable": True, "notReportableReason": None,
}

# Station 2: mid-set, three games in, character switch mid-set visible.
station2 = {
    "id": to_id(now - 360), "station": 2, "ingestedAt": now - 20,
    "set": {"setId": to_id(now - 360), "complete": False, "startEpoch": now - 360,
            "endEpoch": None, "winsRequired": 3, "matchCount": 3,
            "winnerSlot": None, "winnerName": None,
            "players": [{"slot": 0, "name": "BRUJITA", "character": "Maypul", "wins": 2},
                        {"slot": 1, "name": "JUGZ!", "character": "Wrastor", "wins": 1}],
            "games": [game(1, 0, "Maypul", "Clairen"),
                      game(2, 1, "Maypul", "Clairen"),
                      game(3, 0, "Maypul", "Wrastor")]},
    "matchedStartggSetId": "sgg-set-202", "fullRoundText": "Winners Semi-Final",
    "entrants": [{"id": "E3", "name": "Brujita"}, {"id": "E1", "name": "jugeeya"}],
    "slotEntrants": [{"slot": 0, "entrantId": "E3", "entrantName": "Brujita"},
                     {"slot": 1, "entrantId": "E1", "entrantName": "jugeeya"}],
    "candidateWinnerEntrantId": None, "confidence": "none", "status": "live",
    "liveConfirmed": True, "swap": False, "mode": None, "startggState": 2,
    "reportable": True, "notReportableReason": None,
}

# Station 3: finished 3-1, matched with a high-confidence candidate, awaiting
# the Report click.
station3 = {
    "id": to_id(now - 1320), "station": 3, "ingestedAt": now - 180,
    "set": {"setId": to_id(now - 1320), "complete": True, "startEpoch": now - 1320,
            "endEpoch": now - 190, "durationSeconds": 1130, "winsRequired": 3,
            "matchCount": 4, "winnerSlot": 0, "winnerName": "LOOM",
            "winnerCharacter": "Zetterburn",
            "players": [{"slot": 0, "name": "LOOM", "character": "Zetterburn", "wins": 3},
                        {"slot": 1, "name": "KIM", "character": "Kragg", "wins": 1}],
            "games": [game(1, 0, "Ranno", "Kragg"),
                      game(2, 1, "Ranno", "Kragg"),
                      game(3, 0, "Ranno", "Kragg"),
                      game(4, 0, "Zetterburn", "Kragg")]},
    "matchedStartggSetId": "sgg-set-203", "fullRoundText": "Winners Round 3",
    "entrants": [{"id": "E4", "name": "Loom"}, {"id": "E2", "name": "Kimchi"}],
    "slotEntrants": [{"slot": 0, "entrantId": "E4", "entrantName": "Loom"},
                     {"slot": 1, "entrantId": "E2", "entrantName": "Kimchi"}],
    "candidateWinnerEntrantId": "E4", "confidence": "high", "status": "matched",
    "swap": False, "mode": None, "startggState": 2,
    "reportable": True, "notReportableReason": None,
}

state = {
    "version": 7,
    "stations": {slug: {
        "1": {"station": 1, "updatedAt": now - 40, "current": {"state": "set_start"}},
        "2": {"station": 2, "updatedAt": now - 15, "current": {"state": "set_open"}},
        "3": {"station": 3, "updatedAt": now - 200, "current": {"state": "idle"}},
    }},
    "sets": {slug: {
        "1:" + station1["id"]: station1,
        "2:" + station2["id"]: station2,
        "3:" + station3["id"]: station3,
    }},
}
json.dump(state, open(profile + "/hub-state.json", "w"))

# The Current Sets fixture (available-sets.png) — not part of hub state; the
# app seeds it straight into the panel via RSR_SEED_STATE's `availableSets`.
available = {
    "availableSets": {
        "sets": [
            {"id": "sgg-set-300", "state": 2, "fullRoundText": "Winners Quarter-Final",
             "station": 1, "stream": None,
             "entrants": [{"id": "E1", "name": "jugeeya"}, {"id": "E4", "name": "Kimchi"}],
             "startggStartedAt": now - 12 * 60, "startggTotalGames": 5},
            {"id": "sgg-set-301", "state": 1, "fullRoundText": "Winners Round 1",
             "station": 2, "stream": None,
             "entrants": [{"id": "E5", "name": "Loom"}, {"id": "E6", "name": "Rando"}]},
            {"id": "sgg-set-302", "state": 1, "fullRoundText": "Losers Round 2",
             "station": None, "stream": "socalrivals",
             "entrants": [{"id": "E3", "name": "Brujita"}, {"id": "E2", "name": "Kimchi"}]},
        ],
        "stations": [{"number": 1}, {"number": 2}, {"number": 3}],
        "streams": [{"name": "socalrivals"}, {"name": "main-stage"}],
    }
}
json.dump(available, open(profile + "/available-seed.json", "w"))
print("seeded", profile)
