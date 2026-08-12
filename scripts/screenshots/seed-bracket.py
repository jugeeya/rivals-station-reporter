#!/usr/bin/env python3
"""Writes the Bracket screen fixture (bracket-seed.json) into a profile dir.

The Bracket screen reads live from start.gg, which a CI runner can't do and
shouldn't anyway — a shot of a real bracket would change every time someone
played a set. This builds a fixed 12-entrant double-elimination bracket
instead, with the same cast as the other screenshots and the same evening
behind it:

  * Winners is played out to the Semi-Finals; one Semi is live on station 2
    (BRUJITA vs NAVI, the set the operator console shows mid-set).
  * Losers is played through Round 2, with Round 3 seeded and waiting.
  * jugeeya vs Kimchi sits called-but-not-started on station 1, matching the
    console's "Start Match pressed 18 minutes ago" story.
  * The rest of the tree is still waiting on earlier rounds, so the shot
    shows all four card states at once: done, live, called, and empty seats.

The action bar's pickers read the tournament's stations and streams off the
bracket itself (the real fetch asks start.gg for both), so the fixture carries
them too.
"""
import json
import sys

profile, now = sys.argv[1], int(sys.argv[2])
# live (default) | unstarted | done — the three shapes the README shows.
variant = sys.argv[3] if len(sys.argv) > 3 else "live"

COMPLETED, CREATED, ONGOING, CALLED = 3, 1, 2, 6

# 12 entrants: 4 play Winners Round 1, 8 get byes into Round 2. Characters
# are per entrant here; a real bracket reads them per set from game data, so
# a counterpick could differ round to round.
NAMES = {
    "E1": "jugeeya", "E2": "Kimchi", "E3": "BRUJITA", "E4": "NAVI",
    "E5": "LOOM", "E6": "SLADE", "E7": "Rivers", "E8": "Wren",
    "E9": "Marsh", "E10": "Olly", "E11": "KAZE", "E12": "PIP",
}
CHARS = {
    "E1": "Fleet", "E2": "Zetterburn", "E3": "Maypul", "E4": "Fleet",
    "E5": "Zetterburn", "E6": "Kragg", "E7": "Clairen", "E8": "Absa",
    "E9": "Orcane", "E10": "Loxodont", "E11": "Orcane", "E12": "Etalus",
}

sets = []


def add(sid, ident, round_, title, a, b, *, winner=None, score=None,
        state=CREATED, station=None, stream=None, started=None, ended=None,
        feeds=(None, None)):
    """One node.

    `a`/`b` are entrant ids, or None for a seat still waiting. `feeds` names
    the set id filling each seat — that is what the screen draws the
    connector lines from, and what pulls a set level with its feeders.
    """
    def slot(eid, games, feeder):
        if eid is None:
            return {"entrant_id": None, "name": None, "score": None,
                    "character": None, "prereq_set_id": feeder}
        return {"entrant_id": eid, "name": NAMES[eid], "score": games,
                "character": CHARS[eid], "prereq_set_id": feeder}

    sa, sb = (score or (None, None))
    fa, fb = feeds
    sets.append({
        "id": sid, "identifier": ident, "round": round_, "full_round_text": title,
        "state": state, "winner_id": winner, "total_games": 5,
        "started_at": started, "completed_at": ended,
        "station": station, "stream": stream,
        "slots": [slot(a, sa, fa), slot(b, sb, fb)],
    })


m = 60
# ---- winners ---------------------------------------------------------------
add("w-a", "A", 1, "Winners Round 1", "E9", "E10", winner="E9", score=(3, 1),
    state=COMPLETED, station=1, started=now - 95 * m, ended=now - 78 * m)
add("w-b", "B", 1, "Winners Round 1", "E11", "E12", winner="E11", score=(3, 0),
    state=COMPLETED, station=2, started=now - 94 * m, ended=now - 80 * m)

add("w-c", "C", 2, "Winners Quarter-Final", "E1", "E9", winner="E1", score=(3, 2),
    state=COMPLETED, station=1, started=now - 74 * m, ended=now - 52 * m,
    feeds=(None, "w-a"))
add("w-d", "D", 2, "Winners Quarter-Final", "E2", "E11", winner="E2", score=(3, 1),
    state=COMPLETED, station=2, started=now - 73 * m, ended=now - 55 * m,
    feeds=(None, "w-b"))
add("w-e", "E", 2, "Winners Quarter-Final", "E3", "E7", winner="E3", score=(3, 0),
    state=COMPLETED, station=3, started=now - 72 * m, ended=now - 58 * m)
add("w-f", "F", 2, "Winners Quarter-Final", "E4", "E8", winner="E4", score=(3, 2),
    state=COMPLETED, station=3, started=now - 50 * m, ended=now - 31 * m)

# One live, one called and waiting on its players — the two states a TO acts on.
add("w-g", "G", 3, "Winners Semi-Final", "E3", "E4", score=(2, 1),
    state=ONGOING, station=2, stream="socalrivals", started=now - 6 * m,
    feeds=("w-e", "w-f"))
add("w-h", "H", 3, "Winners Semi-Final", "E1", "E2",
    state=CALLED, station=1, started=now - 18 * m, feeds=("w-c", "w-d"))

add("w-i", "I", 4, "Winners Final", None, None, feeds=("w-g", "w-h"))
# The Grand Final's other seat comes from the losers side, which lives in its
# own canvas — the link is recorded but only the winners one is drawable here.
add("w-j", "J", 5, "Grand Final", None, None, feeds=("w-i", "l-h"))

# ---- losers ----------------------------------------------------------------
add("l-a", "K", -1, "Losers Round 1", "E10", "E12", winner="E10", score=(3, 2),
    state=COMPLETED, station=3, started=now - 70 * m, ended=now - 49 * m)

add("l-b", "L", -2, "Losers Round 2", "E7", "E10", winner="E7", score=(3, 1),
    state=COMPLETED, station=1, started=now - 46 * m, ended=now - 29 * m,
    feeds=(None, "l-a"))
add("l-c", "M", -2, "Losers Round 2", "E8", "E5", winner="E5", score=(1, 3),
    state=COMPLETED, station=2, started=now - 44 * m, ended=now - 26 * m)

# Seeded and callable: nobody has assigned these yet.
# Both seats filled, nobody has called them: these render as "ready", the
# state a TO is scanning for. One already sits at a free setup.
add("l-d", "N", -3, "Losers Round 3", "E6", "E7", feeds=(None, "l-b"), station=3)
add("l-e", "O", -3, "Losers Round 3", "E5", "E9", feeds=("l-c", None))

add("l-f", "P", -4, "Losers Quarter-Final", None, None, feeds=("l-d", "l-e"))
add("l-g", "Q", -5, "Losers Semi-Final", None, None, feeds=("l-f", None))
add("l-h", "R", -6, "Losers Final", None, None, feeds=("l-g", None))

seed = {
    "bracket": {
        "event_name": "Rivals 2 Singles",
        "tournament_name": "The Hangout #47",
        # What the action bar's two pickers offer.
        "stations": [1, 2, 3],
        "streams": ["socalrivals"],
        # Pin the selection rather than letting set_needing_attention pick,
        # so the action bar in the shot is always the live Semi-Final.
        "selected": "w-g",
        "sets": sets,
    },
}
if variant == "unstarted":
    # Nothing has been started on start.gg: every set is a placeholder, no
    # scores, no stations. This is what a TO sees before the first call.
    for st in sets:
        st["preview"] = True
        st["state"] = CREATED
        st["station"] = None
        st["stream"] = None
        st["winner_id"] = None
        st["started_at"] = None
        st["completed_at"] = None
        for sl in st["slots"]:
            sl["score"] = None
            sl["character"] = None
    # Only the sets seeded from entry have players before a bracket runs.
    seeded_from_entry = {"w-a", "w-b", "w-c", "w-d", "w-e", "w-f", "l-a"}
    for st in sets:
        if st["id"] not in seeded_from_entry:
            for sl in st["slots"]:
                sl["entrant_id"] = None
                sl["name"] = None
    seed["bracket"]["selected"] = "w-a"

elif variant == "done":
    # Everything played out. Nothing is actionable; the tree is a record.
    finals = {"w-i": ("E3", "E1", (3, 1)), "w-j": ("E3", "E5", (3, 2)),
              "l-f": ("E5", "E6", (3, 0)), "l-g": ("E5", "E9", (3, 1)),
              "l-h": ("E5", "E1", (3, 2))}
    for st in sets:
        st["state"] = COMPLETED
        st["station"] = st["station"] or 1
        if st["id"] in finals:
            w, l, (a, b) = finals[st["id"]]
            st["winner_id"] = w
            st["slots"] = [
                {"entrant_id": w, "name": NAMES[w], "score": a,
                 "character": CHARS[w], "prereq_set_id": st["slots"][0]["prereq_set_id"]},
                {"entrant_id": l, "name": NAMES[l], "score": b,
                 "character": CHARS[l], "prereq_set_id": st["slots"][1]["prereq_set_id"]},
            ]
        elif st["winner_id"] is None:
            st["winner_id"] = st["slots"][0]["entrant_id"]
    # Land on the Grand Final: a played-out bracket's action bar is where a
    # wrong result gets fixed, so the shot should show that being offered.
    seed["bracket"]["selected"] = "w-j"

name = "bracket-seed.json" if variant == "live" else f"bracket-{variant}-seed.json"
json.dump(seed, open(profile + "/" + name, "w"))
print("seeded", variant, "bracket fixture in", profile)
