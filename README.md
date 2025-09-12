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
- CLI commands: init, install, add, list, cache, pm
- Manifest read/write
- Prototype lockfile format=1

### pm utilities
- `pm lockfile -f json|yaml` – print human-readable lockfile
- `pm prune` – prune unreachable transitive dependencies
- `pm ls` – alias for `list`

### Removal behavior
- When running `install` with no package args and packages were removed from `package.json`, pacm now also prunes transitive dependencies from both `node_modules` and the lockfile. When no packages remain, empty scoped folders are cleaned up too.

## TODO (next)
- Semver resolution
- Registry fetch + integrity hashing
- Global content-addressable store & linker

License: MIT OR Apache-2.0
