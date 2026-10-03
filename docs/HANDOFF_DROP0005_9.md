# Handoff — DROP 0005.9 after headless Chaos Pass

DROP 0005.8 is complete.

DROP 0005.9 headless Chaos Pass is complete.

The only remaining DROP 0005 requirement is the fresh Windows/NVIDIA/Vulkan
hardware and visual acceptance defined by the original scope.

## Start here

Branch:

```text
chatgpt/drop-0005-9-chaos
```

Tested headless code head:

```text
41904a9d1919dd85daa2d47541fd9343a3a9f08b
```

GitHub Actions:

```text
run 207
id 37116900462
engine job PASS
DROP 0005 mechanics chaos gate PASS
```

Accepted deterministic evidence:

```text
docs/diagnostics/drop0005_9_chaos/report.json
docs/drop-0005-9.md
docs/drop-0005-8.md
```

Read the accepted report rather than reconstructing chaos outcomes from prose.

## What 0005.8 proved

The post-DROP-0006 engine still satisfies the fantasy participation rule.

Six focused matrix cases cover:

```text
full mechanics
damage only
connectivity only
silent mechanics-disabled world
floating fantasy object override
adjacent policy scopes using identical masonry
```

No production mechanics code change was required.

## What 0005.9 headless proved

The existing `chaos4` framework was extended, not replaced.

Default DROP-4 report remains byte-identical.

DROP-5 mode runs all inherited DROP-4 abuse first plus mechanics-specific cases.

The DROP-5 report was generated twice in one CI run and compared byte-for-byte.

Final report:

```text
cases               24
integrity_verified  true
all_ok              true
```

New mechanics cases include:

```text
massive mixed-material exact solve
overload redistribution
capacity at a region boundary
tiny work/memory/snapshot budgets
stale async capacity worker result
extreme density ratios
mechanical failure storm
24-run deterministic full collapse
fantasy profiled island with stress disabled
mechanics sidecar isolation/integrity
```

Important measured facts:

```text
mixed fixture:
  2,048 cells
  11,264 connections
  2,048 augmentations
  repeatable and bounded

redistribution:
  8 generations
  final 240 cells
  anchors intact

failure storm:
  5 generations
  final 1,728 cells
  anchors intact
  deterministic

full-collapse replay:
  24 identical runs
  8 generations
  final 240 cells

fantasy disabled:
  2,048 floating profiled cells
  128 / 128 attempts NotEvaluated
  world unchanged

stale worker:
  capacity actionable on snapshot
  old commit rejected Stale
  authoritative state unchanged
```

## Hard rules still in force

```text
no physical system required for authored world validity
support/anchoring separate from material identity
participation selected by host scope, not encoded as fake material strength
unknown != empty
budget exhaustion != permission to fail/collapse
mechanics failures re-enter the ordinary destruction pipeline
no second mechanical detachment system
anchored cells never selected as mechanical failures
fixed-point capacity path remains deterministic
exact solver remains oracle beside conservative witness
```

## Remaining acceptance — Windows only

Do not do more headless mechanics work merely because hardware is unavailable.

What remains is evidence, not a missing solver.

On the current Windows/NVIDIA machine, run the current branch through the
existing acceptance/capture path and record:

```text
material-aware failure/collapse scene
current visual output
frame p50 / p95 / p99 / max
frames over 33.3 ms
camera/movement responsiveness
current relevant destruction response latency
```

Capture must use the existing Vulkan-safe non-invasive path (`ddagrab` or
native render-target capture as appropriate).

Do not call software-Vulkan timing equivalent hardware evidence.

## Infrastructure

The user wants this free.

Use GitHub Actions for headless work.

Do not provision DigitalOcean unless explicitly requested later.

Desktop Commander quota was exhausted at the time of this handoff, so Windows
access may still be unavailable. If unavailable, stop at this boundary rather
than replacing the hardware gate with another Linux test.

## After hardware acceptance

If the current Windows/NVIDIA run passes, close DROP 0005 as complete and write
a final DROP 0005 closeout.

Do not start another major subsystem before that closeout.
