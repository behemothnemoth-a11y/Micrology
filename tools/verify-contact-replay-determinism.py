#!/usr/bin/env python3
"""Compare authoritative checkpoints from repeated contact-volley replays.

Backend poses and wall-clock/frame timing are deliberately excluded. The proof
is about topology and deterministic scheduling: the same fixed-step script must
produce the same structural/fracture state and the same bounded contact/queue
outcomes however quickly render frames happen to retire.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

EXPECTED_LABELS = (
    "volley_intact",
    "volley_ready",
    "volley_first_contact",
    "volley_after_120",
    "volley_second",
    "volley_damage_off",
)

CONTACT_KEYS = (
    "samples",
    "enqueued",
    "processed",
    "refused",
    "stale",
    "capped",
    "pending",
    "static_failed",
    "fragment_failed",
    "broken_bonds",
    "fragments_created",
    "fragment_fragment_pairs",
    "work",
)

BACKGROUND_KEYS = (
    "accepted",
    "failed_cells",
    "stale",
    "refused",
    "capped",
    "cut_failed",
    "capacity_failed",
    "generation",
    "max_queue",
)


def load_run(path: Path) -> dict[str, dict]:
    checkpoints: dict[str, dict] = {}
    for candidate in sorted(path.glob("*.json")):
        document = json.loads(candidate.read_text(encoding="utf-8"))
        label = document.get("label")
        if label in EXPECTED_LABELS:
            if label in checkpoints:
                raise SystemExit(f"{path}: duplicate checkpoint {label}")
            checkpoints[label] = document
    missing = [label for label in EXPECTED_LABELS if label not in checkpoints]
    if missing:
        raise SystemExit(f"{path}: missing checkpoints: {', '.join(missing)}")
    return checkpoints


def selected(document: dict) -> dict:
    contacts = document.get("contacts") or {}
    background = document.get("background") or {}
    fracture = document["fracture"]
    replay = document.get("replay") or {}
    return {
        "label": document["label"],
        "fixed_ticks": document["fixed_ticks"],
        "contact_fracture_enabled": document["contact_fracture_enabled"],
        "structural": document["structural"],
        "fracture": {
            "cell_entries": fracture["cell_entries"],
            "bond_entries": fracture["bond_entries"],
            "broken_bonds": fracture["broken_bonds"],
            "revision": fracture["revision"],
            "checksum_fnv1a64": fracture["checksum_fnv1a64"],
        },
        "contacts": {key: contacts.get(key) for key in CONTACT_KEYS},
        "background": {key: background.get(key) for key in BACKGROUND_KEYS},
        "last_separation": document.get("last_separation"),
        "replay": {
            "name": replay.get("name"),
            "cursor": replay.get("cursor"),
            "commands": replay.get("commands"),
            "blocked_case": replay.get("blocked_case"),
        },
    }


def main() -> int:
    runs = [Path(arg) for arg in sys.argv[1:]]
    if len(runs) < 2:
        raise SystemExit("usage: verify-contact-replay-determinism.py RUN_DIR RUN_DIR [...]")

    loaded = [load_run(run) for run in runs]
    reference = {
        label: selected(loaded[0][label])
        for label in EXPECTED_LABELS
    }

    for run_index, run in enumerate(loaded[1:], start=2):
        for label in EXPECTED_LABELS:
            actual = selected(run[label])
            expected = reference[label]
            if actual != expected:
                print(
                    json.dumps(
                        {
                            "status": "MISMATCH",
                            "run": run_index,
                            "label": label,
                            "expected": expected,
                            "actual": actual,
                        },
                        indent=2,
                        sort_keys=True,
                    )
                )
                return 1

    final = reference["volley_damage_off"]
    print(
        json.dumps(
            {
                "status": "PASS",
                "runs": len(runs),
                "checkpoints_per_run": len(EXPECTED_LABELS),
                "final_fixed_ticks": final["fixed_ticks"],
                "final_structural": final["structural"],
                "final_fracture": final["fracture"],
                "final_contacts": final["contacts"],
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
