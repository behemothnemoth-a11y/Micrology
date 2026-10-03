# DROP 0007 — Editor Foundation

Status: **in progress**

Roadmap phase: **D — Editor**

DROP 0001–0006 built the engine substrate the editor is allowed to use: native
cell worlds, exact picking, transactional edits, streaming, persistence,
destruction, fragments, material mechanics and deterministic replay.

DROP 0007 turns those systems into an authoring surface without moving editor
state into the world model and without teaching engine crates about Bevy.

The long-term roadmap names:

```text
selection
transform gizmos
paint / carve / add
layers
material browser
object / region inspector
play / stop
```

This drop establishes the engine-side contracts first, then adds a desktop
adapter on top.

## Hard rules

### The editor is a client of the engine

The editor must call the same world APIs as every other host.

It may not:

- edit volume internals directly;
- invent a second world mutation format;
- bypass dirty tracking;
- bypass revisions;
- treat unloaded regions as empty;
- make render meshes authoritative;
- make physics backend state authoritative.

### Editor core stays Bevy-free

Reusable selection, commands, history and authoring metadata belong in an
engine crate.

Bevy/UI/input/gizmo rendering belong in the desktop host.

### Undo is exact, not approximate

Undo/redo must restore exact cell/material state in canonical order.

Commands created against stale world state must refuse before mutation.

A refused command may not partially edit the world.

### History is bounded in bytes

History may not grow without limit.

A single command too large for the configured history ceiling refuses before
mutation. Older undo entries may be evicted deterministically to admit a new
command.

### Editing does not silently turn physics on

Authoring a floating island remains valid.

An edit returns ordinary `EditOutcome`; the host decides whether any optional
structural/destruction system participates.

## Existing engine seams to reuse

Already implemented and permanent:

```text
engine_world::raycast
  exact cell picking from native world data

WorldEditBatch
  canonical transactional cell writes

World::apply
  revisions, dirty tracking, seam invalidation, structural candidates,
  unknown-neighbour reporting

MaterialRegistry / MechanicalRegistry
  existing material identities and optional mechanics side tables
```

DROP 0007 must build on these, not replace them.

---

## 0007.0 — command and history kernel

Create a small Bevy-free `engine_editor` crate.

First contracts:

- canonical captured cell-edit command;
- forward and inverse batches;
- stale validation before execute/undo/redo;
- exact undo/redo;
- redo invalidation after a new edit;
- bounded history by entry count and tracked bytes;
- deterministic oldest-first history eviction;
- oversize-command refusal before world mutation;
- normal `EditOutcome` returned from every accepted mutation.

No selection UI yet.

No transform gizmo yet.

No clipboard yet.

No support/anchor editing yet.

Acceptance must include:

- edits supplied in different order produce the same command;
- no-op writes disappear from the command;
- execute then undo returns the exact original world;
- undo then redo returns the exact edited world;
- stale execute refuses atomically;
- stale undo/redo refuse atomically;
- a new edit clears redo;
- history byte ceiling is deterministic;
- an oversize command changes nothing;
- boundary edits still dirty the touching neighbour through `World::apply`.

---

## 0007.1 — selection model

Engine-native selection vocabulary:

- single cell;
- inclusive box;
- additive/subtractive selection;
- canonical selected bounds;
- selection size/count;
- safe behaviour at coordinate extremes;
- selection independent of render entities.

Viewport picking continues to use `engine_world::raycast`.

---

## 0007.2 — authoring operations

Selection-driven operations that compile to ordinary commands:

- fill/add;
- carve/remove;
- repaint;
- copy/paste of cell/material payload;
- translate copied/selected payload with checked coordinates.

Every operation must produce a command before mutation so it is undoable and
stale-checkable.

---

## 0007.3 — desktop editor mode

Add an opt-in editor mode to the desktop host rather than turning the sandbox
into a monolithic editor.

Initial surface:

- editor/play mode switch;
- visible selection;
- material picker;
- add/carve/paint tools;
- undo/redo;
- simple transform interaction using engine selection/commands.

All Bevy-specific code remains in the host.

---

## 0007.4 — region/object inspector

Expose engine truth without making the inspector authoritative:

- global cell/region coordinates;
- resident/dirty state;
- material identity;
- region/volume revision;
- support/anchor state;
- optional mechanical profile;
- optional participation policy chosen by the host;
- current selection statistics.

No debug panel may mutate engine internals directly.

---

## 0007.5 — layers / authoring metadata

Define authoring layers separately from material and physical participation.

Layer membership must not become:

- material identity;
- support identity;
- residency identity;
- physics participation.

Determine persistence/versioning only after the engine-side layer model is
proven.

---

## 0007.6 — play / stop boundary

Prove the editor can enter simulation and return to authored state deliberately.

The exact policy for simulation changes must be explicit:

- discard;
- keep;
- or snapshot/branch.

Never silently mix runtime destruction into authored state.

---

## 0007.7 — acceptance and Chaos Pass

Headless acceptance:

- large selections;
- edit storms;
- undo/redo storms;
- stale commands;
- coordinate extremes;
- history exhaustion;
- region-boundary edits;
- edit → save → reload equivalence;
- fantasy geometry remains valid with physical participation disabled;
- live target unchanged after disposable chaos abuse.

Windows/GPU acceptance when hardware is available:

- viewport picking;
- gizmo interaction;
- editor camera;
- material/tool UI;
- visible selection;
- responsive undo/redo;
- frame/hitch measurements.

Software Vulkan is not a substitute for final interaction/GPU evidence.

## Current infrastructure

The user wants to keep infrastructure free.

Use:

```text
GitHub branches
GitHub Actions for engine/headless acceptance
Windows/NVIDIA machine only when visual/input/GPU evidence is required
```

Do not provision DigitalOcean unless explicitly requested later.

The deferred DROP 0005 Windows/NVIDIA acceptance remains recorded separately.
Starting DROP 0007 does not retroactively mark that evidence complete.
