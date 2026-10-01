# Suggested Initial Repository Skeleton

Claude may adjust this if a simpler layout makes DROP 0001 cleaner.

```text
/
├── Cargo.toml
├── README.md
├── LICENSE
├── rust-toolchain.toml            # optional
├── .gitignore
│
├── crates/
│   ├── engine_core/
│   │   └── src/
│   ├── engine_world/
│   │   └── src/
│   ├── engine_volume/
│   │   └── src/
│   ├── engine_geometry/
│   │   └── src/
│   └── engine_io/
│       └── src/
│
├── apps/
│   └── sandbox/
│       └── src/
│
├── fixtures/
│   ├── volumes/
│   └── worlds/
│
├── docs/
│   ├── architecture.md
│   ├── native-world-format.md
│   └── testing.md
│
└── scripts/
```

## Suggested minimal ownership

### engine_core
- shared IDs
- coordinates
- revisions
- small common utilities

### engine_volume
- cell storage
- local volume
- material IDs
- palette
- edit API

### engine_world
- chunks
- mapping world coordinates to local volumes/chunks
- dirty tracking

### engine_geometry
- exact surface extraction
- optional greedy mesher
- deterministic mesh output

### engine_io
- versioned save/load

### sandbox
- Bevy application
- camera
- upload compiled mesh to renderer
- basic picking/edit controls

If this split causes unnecessary ceremony, combine crates. The architectural boundaries matter more than crate count.
