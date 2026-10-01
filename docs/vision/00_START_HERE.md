# Standalone Volumetric Engine — Claude Transfer Pack v1

## What this project is

This is a new standalone game-engine repository.

The long-term goal is a reusable engine for multiple games built around:

- large streamed worlds
- destructible volumetric geometry
- very fine local detail without requiring fine detail everywhere
- procedural world generation
- hand-authored worlds
- hybrid authored/generated worlds
- modular gameplay projects
- Minecraft import/export compatibility
- future GIS / real-world worldbuilding experiments
- future No Man's Sky-style deterministic world generation

This is **not** a Minecraft mod.

Minecraft is one source/target format and a test ecosystem, not the engine foundation.

## Where the idea comes from

The user's existing `astra-microblocks` project proved several useful ideas:

- a 16x16x16 local microcell volume
- mixed materials inside one volume
- native RGB materials
- deterministic greedy face merging
- exact geometry preservation
- transformations
- reusable sculptures
- cached physical occupancy
- schematic compatibility
- regression fixtures and geometry verification

The standalone engine should reuse the proven *ideas and tests*, but avoid Minecraft-specific architecture.

## Immediate instruction to Claude

Start the new repo and build only the basic framework described in `03_DROP_0001_SCOPE.md`.

Do not jump ahead into:
- procedural planets
- combat
- complex physics
- structural destruction
- networking
- multiplayer
- real-world GIS ingestion
- full editor tooling
- No Man's Sky-scale generation
- content generation

The first milestone is deliberately small:

> Prove that the engine can own, display, edit, save, and reload engine-native volumetric geometry efficiently without Minecraft.

Read the files in this package in numeric order.
