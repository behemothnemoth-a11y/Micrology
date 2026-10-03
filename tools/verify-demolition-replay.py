"""Check real host dumps, including refusal safety and all surviving material.

Usage: python tools/verify-demolition-replay.py [diagnostic-directory]
"""
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/diagnostics/destruction_lab")


def load(label):
    paths = list(root.glob(label + "-tick*.json"))
    assert len(paths) == 1, (label, paths)
    value = json.loads(paths[0].read_text(encoding="utf-8"))
    assert value["paused"]
    assert not value["background"]["active"]
    assert value["background"]["pending"] == 0
    assert value["contacts"]["pending"] == 0
    assert value["interaction"]["pending"] == 0
    return value


def mass(value, initial):
    geometry = value["structural"]
    assert geometry["static_cells"] + geometry["fragment_cells"] + value["background"]["failed_cells"] == initial, value["label"]
    assert sum(f["cells"] for f in value["fragment_dynamics"]) == geometry["fragment_cells"]


labels = ["intact", "one_support", "overloaded_off", "roof_released", "roof_impact", "lifted", "pulled", "slammed", "settled", "systems_off"]
values = {label: load("demolition_" + label) for label in labels}
for value in values.values():
    mass(value, 1811)
    assert value["background"]["refused"] == 0
    assert value["background"]["stale"] == 0
assert values["intact"]["background"]["capacity_status"] == "Sound"
assert values["one_support"]["background"]["capacity_status"] == "Sound"
assert values["overloaded_off"]["structural"]["fragments"] == 0
assert values["overloaded_off"]["structural"]["static_cells"] == 1803
assert not values["overloaded_off"]["background"]["capacity_enabled"]
assert values["roof_released"]["structural"]["fragment_cells"] == 682
assert values["roof_released"]["structural"]["fragments"] == 1
assert values["roof_released"]["background"]["capacity_failed"] == 8
assert values["pulled"]["interaction"]["hold_steps"] == 180
assert values["pulled"]["structural"] == values["roof_released"]["structural"]
assert values["slammed"]["interaction"]["throws"] == 1
assert values["slammed"]["contacts"]["broken_bonds"] > values["roof_impact"]["contacts"]["broken_bonds"]
assert values["settled"]["structural"]["fragments"] > 1
assert values["settled"]["interaction"]["applied"] > 0
assert values["settled"]["structural"] == values["systems_off"]["structural"]
assert values["settled"]["fracture"] == values["systems_off"]["fracture"]
assert not values["systems_off"]["contact_fracture_enabled"]
assert not values["systems_off"]["background"]["capacity_enabled"]

if list(root.glob("demolition_stress_18-tick*.json")):
    first = load("demolition_stress_9")
    second = load("demolition_stress_18")
    off = load("demolition_stress_off")
    mass(first, 1811 + 9 * 60)
    mass(second, 1811 + 18 * 60)
    mass(off, 1811 + 18 * 60)
    assert second["structural"] == off["structural"]
    assert second["fracture"] == off["fracture"]
    print("Stress mass accounting and systems-off preservation passed.")
print("Demolition support, collapse, grab, collision cracks, blast and systems-off checks passed.")
