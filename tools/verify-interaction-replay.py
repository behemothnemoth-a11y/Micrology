"""Validate real sandbox dumps; never synthesize physics or expected evidence.

Usage: python tools/verify-interaction-replay.py [dump-directory]
"""
import json
import math
import sys
from pathlib import Path

directory = Path(sys.argv[1] if len(sys.argv) > 1 else "target/diagnostics/destruction_lab")


def read(label, tick):
    value = json.loads((directory / f"{label}-tick{tick:06}.json").read_text(encoding="utf-8"))
    assert value["fixed_ticks"] == tick
    bodies = value["fragment_dynamics"]
    assert len({body["id"] for body in bodies}) == len(bodies)
    assert sum(body["cells"] for body in bodies) == value["structural"]["fragment_cells"]
    for body in bodies:
        assert body["cells"] > 0
        assert all(math.isfinite(x) for key in ["translation", "rotation_xyzw", "world_center", "linear_velocity", "angular_velocity"] for x in body[key])
    return value


intact = read("tools_intact", 0)
lifted = read("tools_lifted", 120)
pulled = read("tools_pulled", 210)
thrown = read("tools_thrown", 330)
separated = read("tools_blast_separated", 330)
settled = read("tools_blast_settled", 510)
disabled = read("tools_simulation_damage_off", 630)
assert intact["structural"]["static_cells"] == 7228
assert intact["structural"]["static_anchors"] == 4348
assert lifted["structural"] == pulled["structural"]
assert lifted["fracture"] == pulled["fracture"]
assert lifted["structural"]["fragments"] == 1
assert lifted["structural"]["fragment_cells"] == 60
errors = []
for stage, target in [(lifted, [64.5, 16, 78]), (pulled, [68.5, 14, 75])]:
    center = stage["fragment_dynamics"][0]["world_center"]
    error = math.dist(center, target)
    assert error < 0.4, (center, target, error)
    errors.append(error)
assert pulled["interaction"]["hold_steps"] == 210
assert thrown["interaction"]["throws"] == 1
assert thrown["interaction"]["held"] is None
assert thrown["contacts"]["processed"] > 0
assert thrown["structural"] != pulled["structural"]
assert separated["interaction"]["broken_bonds"] > 0
assert separated["interaction"]["fragments_created"] > 0
assert separated["interaction"]["kicks"] > 1
assert separated["interaction"]["pending"] == 0
assert separated["interaction"]["refused"] == 0
assert settled["contacts"]["processed"] > thrown["contacts"]["processed"]
for stage in [thrown, separated, settled, disabled]:
    c, i, s = stage["contacts"], stage["interaction"], stage["structural"]
    assert s["static_cells"] + s["fragment_cells"] + c["static_failed"] + c["fragment_failed"] + i["failed_cells"] == 7228 + 60, "lost or duplicated material"
assert disabled["structural"] == settled["structural"]
assert disabled["fracture"] == settled["fracture"]
assert disabled["contacts"] == settled["contacts"]
assert disabled["contact_fracture_enabled"] is False
print(json.dumps({"verified": True, "hold_error_cells": errors, "fixed_steps": 630,
    "conserved_authored_plus_spawned_cells": 7288,
    "blast": separated["interaction"], "final_structure": disabled["structural"],
    "contacts": disabled["contacts"]}, indent=2))
