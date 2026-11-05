"""Same-day ordering test: transactions, then envelope-assign, then envelope-move; file order breaks ties."""
import contextlib
import copy
import importlib.util
import io
import itertools
from decimal import Decimal as D

import os
HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location(
    "sim4", os.path.join(HERE, "sim4.py"))
sim4 = importlib.util.module_from_spec(spec)
with contextlib.redirect_stdout(io.StringIO()):
    spec.loader.exec_module(sim4)   # re-runs the v4 checks; output suppressed

ORDER = {"tx": 0, "assign": 1, "move": 2}


def run(budget, events, watch=None):
    """events: list of (date, kind, fn) in FILE order. Apply by (date, kind order, file index)."""
    mins = []
    ordered = sorted(enumerate(events), key=lambda t: (t[1][0], ORDER[t[1][1]], t[0]))
    for _, (_, _, fn) in ordered:
        fn(budget)
        if watch:
            mins.append(budget.avail(watch))
    return mins


def state(b):
    return (b.avail("Dining"), b.split("Dining"), b.pay(), b.unassigned())


# Base: the Example 5 state, then the Nov 5 card purchase (Dining -45.00 credit overspend)
base = copy.deepcopy(sim4.base)
base.spend_card("Dining", 200)

nov6_tx = ("2026-11-06", "tx", lambda b: b.spend_cash("Dining", 30))
nov6_assign = ("2026-11-06", "assign", lambda b: b.assign("Dining", 40))

# Both file orders give the same result, and the assignment acts as a correction
results = []
for evs in itertools.permutations([nov6_tx, nov6_assign]):
    b = copy.deepcopy(base)
    run(b, list(evs))
    results.append(state(b))
assert results[0] == results[1]
av, sp, pay, unassigned = results[0]
assert av == -35 and sp == (D(0), D(35)) and pay == 165 and unassigned == 1060
print("same-day, either file order:", results[0])

# Dated earlier (before the Nov 5 purchase), the same 40.00 counts as funds
early = copy.deepcopy(sim4.base)
run(early, [("2026-11-01", "assign", lambda b: b.assign("Dining", 40)),
            ("2026-11-05", "tx", lambda b: b.spend_card("Dining", 200)),
            ("2026-11-06", "tx", lambda b: b.spend_cash("Dining", 30))])
av, sp, pay, unassigned = state(early)
assert av == -35 and sp == (D(30), D(5)) and pay == 195 and unassigned == 1060
print("assigned Nov 1:", state(early))

# Assign before move: the source envelope never looks short
move_events = [
    ("2026-11-10", "move", lambda b: b.move("Fun", "Groceries", 30)),
    ("2026-11-10", "assign", lambda b: b.assign("Fun", 50)),
]
finals = []
for evs in itertools.permutations(move_events):
    b = copy.deepcopy(sim4.base)
    b.cat("Fun")
    mins = run(b, list(evs), watch="Fun")
    assert min(mins) >= 0, "source envelope went negative mid-day"
    finals.append((b.avail("Fun"), b.avail("Groceries"), b.unassigned()))
assert finals[0] == finals[1]
assert finals[0][0] == 20 and finals[0][1] == 10   # Groceries -20 + 30 = 10
print("assign then move:", finals[0])
print("ALL CHECKS PASSED")
