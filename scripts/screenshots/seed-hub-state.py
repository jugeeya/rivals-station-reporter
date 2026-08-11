#!/usr/bin/env python3
"""Writes the operator fixtures (hub-state.json + available-seed.json) into a
profile dir for the README screenshots. One coherent story, no player in two
places at once:

  * Station 1 — jugeeya vs Kimchi just called (Winners Quarter-Final), the
    TO pressed Start Match 18 minutes ago on a best-of-7 (start.gg's
    startedAt/totalGames override the station's own guesses: "18m",
    "first to 4"); no games played yet.
  * Station 2 — BRUJITA vs NAVI mid-set (Winners Semi-Final), 2-1 after
    three games, NAVI switching Clairen -> Fleet in game 3.
  * Station 3 — LOOM vs SLADE just finished 3-1 (Winners Round 3), matched
    with a high-confidence candidate; the station sits idle now. Auto-report
    would have taken this one already at a real event (it reports the moment
    a set is unambiguous); it is left awaiting so the shot can show the
    actions a set carries — Report, edit result, switch players.

Current Sets mirrors the same bracket: both live sets under "playing now"
with the same start times, and two startable sets whose FOUR players are all
different people from the six above (someone mid-set can't also be startable
elsewhere) — one pre-assigned to the now-free station 3, one on the
socalrivals stream with no station yet.
"""
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


station1 = {
    "id": to_id(now - 1080), "station": 1, "ingestedAt": now - 30,
    "set": {"setId": to_id(now - 1080), "complete": False, "startEpoch": now - 720,
            "endEpoch": None, "winsRequired": 3, "matchCount": 0,
            "winnerSlot": None, "winnerName": None,
            "players": [], "games": []},
    "matchedStartggSetId": "sgg-set-201", "fullRoundText": "Winners Quarter-Final",
    "entrants": [{"id": "E1", "name": "jugeeya"}, {"id": "E2", "name": "Kimchi"}],
    "slotEntrants": None,
    "candidateWinnerEntrantId": None, "confidence": "none", "status": "live",
    "liveConfirmed": False, "swap": False, "mode": None, "startggState": 2,
    "startggStartedAt": now - 1080, "startggTotalGames": 7,
    "reportable": True, "notReportableReason": None,
}

station2 = {
    "id": to_id(now - 360), "station": 2, "ingestedAt": now - 20,
    "set": {"setId": to_id(now - 360), "complete": False, "startEpoch": now - 360,
            "endEpoch": None, "winsRequired": 3, "matchCount": 3,
            "winnerSlot": None, "winnerName": None,
            "players": [{"slot": 0, "name": "BRUJITA", "character": "Maypul", "wins": 2},
                        {"slot": 1, "name": "NAVI", "character": "Fleet", "wins": 1}],
            "games": [game(1, 0, "Maypul", "Clairen"),
                      game(2, 1, "Maypul", "Clairen"),
                      game(3, 0, "Maypul", "Fleet")]},
    "matchedStartggSetId": "sgg-set-202", "fullRoundText": "Winners Semi-Final",
    "entrants": [{"id": "E3", "name": "Brujita"}, {"id": "E4", "name": "Navi"}],
    "slotEntrants": [{"slot": 0, "entrantId": "E3", "entrantName": "Brujita"},
                     {"slot": 1, "entrantId": "E4", "entrantName": "Navi"}],
    "candidateWinnerEntrantId": None, "confidence": "none", "status": "live",
    "liveConfirmed": True, "swap": False, "mode": None, "startggState": 2,
    "startggStartedAt": now - 360, "startggTotalGames": 5,
    "reportable": True, "notReportableReason": None,
}

station3 = {
    "id": to_id(now - 1240), "station": 3, "ingestedAt": now - 22,
    "set": {"setId": to_id(now - 1240), "complete": True, "startEpoch": now - 1240,
            "endEpoch": now - 30, "durationSeconds": 1210, "winsRequired": 3,
            "matchCount": 4, "winnerSlot": 0, "winnerName": "LOOM",
            "winnerCharacter": "Zetterburn",
            "players": [{"slot": 0, "name": "LOOM", "character": "Zetterburn", "wins": 3},
                        {"slot": 1, "name": "SLADE", "character": "Kragg", "wins": 1}],
            "games": [game(1, 0, "Ranno", "Kragg"),
                      game(2, 1, "Ranno", "Kragg"),
                      game(3, 0, "Ranno", "Kragg"),
                      game(4, 0, "Zetterburn", "Kragg")]},
    "matchedStartggSetId": "sgg-set-203", "fullRoundText": "Winners Round 3",
    "entrants": [{"id": "E5", "name": "Loom"}, {"id": "E6", "name": "Slade"}],
    "slotEntrants": [{"slot": 0, "entrantId": "E5", "entrantName": "Loom"},
                     {"slot": 1, "entrantId": "E6", "entrantName": "Slade"}],
    "candidateWinnerEntrantId": "E5", "confidence": "high", "status": "matched",
    "swap": False, "mode": None, "startggState": 2,
    "reportable": True, "notReportableReason": None,
}

state = {
    "version": 7,
    "stations": {slug: {
        "1": {"station": 1, "updatedAt": now - 40, "current": {"state": "set_start"}},
        "2": {"station": 2, "updatedAt": now - 15,
              "current": {"state": "set_open", "setId": station2["id"], "matchCount": 3}},
        "3": {"station": 3, "updatedAt": now - 200, "current": {"state": "idle"}},
    }},
    "sets": {slug: {
        "1:" + station1["id"]: station1,
        "2:" + station2["id"]: station2,
        "3:" + station3["id"]: station3,
    }},
}
json.dump(state, open(profile + "/hub-state.json", "w"))

# Current Sets — the same bracket, from start.gg's own point of view.
available = {
    "availableSets": {
        "sets": [
            {"id": "sgg-set-201", "state": 2, "fullRoundText": "Winners Quarter-Final",
             "station": 1, "stream": None,
             "entrants": [{"id": "E1", "name": "jugeeya"}, {"id": "E2", "name": "Kimchi"}],
             "startggStartedAt": now - 1080, "startggTotalGames": 7},
            {"id": "sgg-set-202", "state": 2, "fullRoundText": "Winners Semi-Final",
             "station": 2, "stream": None,
             "entrants": [{"id": "E3", "name": "Brujita"}, {"id": "E4", "name": "Navi"}],
             "startggStartedAt": now - 360, "startggTotalGames": 5},
            {"id": "sgg-set-204", "state": 1, "fullRoundText": "Winners Round 1",
             "station": 3, "stream": None,
             "entrants": [{"id": "E7", "name": "Rivers"}, {"id": "E8", "name": "Wren"}]},
            {"id": "sgg-set-205", "state": 1, "fullRoundText": "Losers Round 2",
             "station": None, "stream": "socalrivals",
             "entrants": [{"id": "E9", "name": "Marsh"}, {"id": "E10", "name": "Olly"}]},
        ],
        "stations": [{"number": 1}, {"number": 2}, {"number": 3}],
        "streams": [{"name": "socalrivals"}, {"name": "main-stage"}],
    }
}
json.dump(available, open(profile + "/available-seed.json", "w"))
print("seeded", profile)
