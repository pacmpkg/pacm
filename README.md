# pacm (Prototype – Rust Edition)

Prototype fast, disk-efficient, secure JavaScript/TypeScript package manager. Original concept targeted Zig; current prototype uses Rust for rapid iteration. See `PROJECT_INFO.md` for full roadmap.

Status: PROTOTYPE – Not production ready.

## Quick Start
```powershell
cargo build --release
target\release\pacm init --name demo --version 0.1.0
target\release\pacm add lodash
target\release\pacm install
target\release\pacm list
```

## Implemented (minimal)
- CLI commands: init, install, add, list
- Manifest read/write
- Prototype lockfile format=1

## TODO (next)
- Semver resolution
- Registry fetch + integrity hashing
- Global content-addressable store & linker

License: MIT OR Apache-2.0
