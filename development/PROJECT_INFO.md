# pacm Project Information

Fast, disk-efficient, secure JavaScript/TypeScript package manager written in Rust.

## Vision
Provide an NPM / pnpm compatible developer experience with:
- pnpm-style global, content-addressable store to eliminate duplicate package data
- Ultra-fast installs via parallel fetch + streaming extraction + link phase
- Deterministic, minimal lockfile format (`pacm.lockb`, binary) for reproducible builds
- Strong integrity & (future) signature verification for supply‑chain security

## Core Principles
1. Speed: end-to-end install dominated by network & disk, not CPU.
2. Determinism: identical inputs => identical node_modules & lockfile.
3. Disk Efficiency: single global copy of each (package, version, integrity) tuple.
4. Security: integrity hashing, path traversal defenses, sandboxed extraction.
5. Transparency: simple, documented lockfile & store layout.
6. Extensibility: modular resolver/fetcher/linker pipeline.

## High-Level Architecture
```
CLI (pacm)
  ├─ Config Loader (env, pacmrc TBD)
  ├─ Package Manifest IO (package.json)
  ├─ Lockfile Engine (pacm.lockb, binary)
  ├─ Dependency Resolver (semver -> concrete versions)
  ├─ Fetch Orchestrator
  │    ├─ Registry API Client (npm registry REST)
  │    ├─ Tarball Fetcher (HTTP GET, streaming)
  │    └─ Integrity Validator (SHA512)
  ├─ Global Store (content-addressable)
  │    ├─ Path: %LOCALAPPDATA%/pacm/store/v1/<algo>/<hash>/package
  │    └─ Metadata index (manifest cache, integrity map)
  ├─ Linker
  │    ├─ Builds virtual dependency graph
  │    ├─ Creates node_modules structure via symlink / junction / hardlink
  │    └─ Hoisting / de-duping strategy (minimal depth, preserve peer deps)
  └─ Audit & Security (future)
```

### Store Layout (proposed)
```
<storeRoot>/v1/sha512/<first2>/<full-hash>/package/  (extracted files)
<storeRoot>/v1/sha512/<first2>/<full-hash>/meta.json (metadata)
```
`meta.json` contains name, version, integrity, engines, dependencies hash.

### node_modules Linking Strategy
- Prefer directory symlinks (Windows: junctions) to avoid copying.
- Fallback to hardlinks when symlinks not permitted.
- Deterministic path mapping: project/.pacm-maps.json (optional trace file).

## Current Prototype Status
| Feature | Status |
|---------|--------|
| CLI scaffold (help/init/install/list) | Implemented (prototype) |
| package.json read/write | Basic (no scripts/fields beyond core) |
| Lockfile write/read | Basic (flat list – no transitive tree yet) |
| Add dependency (no network) | Placeholder only |
| Global store | Not implemented |
| Symlink/link phase | Not implemented |
| Semver parsing & resolution | Not implemented |
| Registry fetching (npm) | Not implemented |
| Integrity (SHA512) | Not implemented |
| Peer / optional / dev deps | Not implemented |
| Removal / prune / update | Not implemented |
| Security checks (path traversal, audit) | Not implemented |

## Roadmap
### Phase 1: Core Resolution & Fetch
- Semver parser & range matcher
- Registry client (https://registry.npmjs.org/<name>)
- Caching of metadata (etag / last-modified)
- Concrete version selection + dedupe
- Streaming tarball extraction to temp + integrity hash during stream

### Phase 2: Global Store & Linking
- Content-addressable store keyed by integrity hash
- Concurrency-safe acquisition (lock file / atomic rename)
- Symlink / junction creation algorithm with hoisting policy
- Deterministic lockfile graph representation (list of packages + dependency edges)

### Phase 3: Dependency Classes & Scripts
- devDependencies, peerDependencies, optionalDependencies
- Lifecycle scripts execution sandbox
- Cross-platform env normalization

### Phase 4: Performance Enhancements
- Parallel fetch with bounded worker pool
- Incremental installs (only new / changed subgraph)
- Lazy extraction (on-demand) experiment
- Bloom / fast existence checks for store hits

### Phase 5: Security & Trust
- Integrity mandatory (SHA512)
- Tarball path sanitization (no absolute or .. components)
- (Future) Sigstore / TUF-inspired signature layer
- Audit ingestion (npm advisory DB) with severity filtering

### Phase 6: Advanced Features
- Workspaces / monorepos
- Plug'n'Play / virtual fs experiment toggle
- Offline & mirror modes
- Store GC & health commands

## Lockfile (Future Schema Draft)
```jsonc
{
  "format": 2,
  "packages": {
    "": {                 // root project
      "name": "my-app",
      "version": "1.0.0",
      "dependencies": {
        "axios": "^1.7.0"
      }
    },
    "node_modules/axios": {
      "version": "1.7.2",
      "integrity": "sha512-...",
      "resolved": "https://registry.npmjs.org/axios/-/axios-1.7.2.tgz",
      "dependencies": {
        "follow-redirects": "^1.15.0"
      }
    },
    "node_modules/follow-redirects": {
      "version": "1.15.6",
      "integrity": "sha512-..."
    }
  }
}
```

## Command Design (Planned)
| Command | Purpose | Notes |
|---------|---------|-------|
| pacm init | Create manifest | Interactive flags later |
| pacm install [pkg...] | Install all or add specific | Adds & resolves |
| pacm add <pkg[@range]> | Alias for install single | Writes manifest & lock |
| pacm remove <pkg> | Remove pkg & prune graph | Peer safety checks |
| pacm update [pkg] | Bump within ranges | Lockfile diff |
| pacm list | Show tree / flattened view | Depth filtering |
| pacm store gc | Reclaim unreferenced hashes | Reference counting |
| pacm audit | Security report | Advisories |
| pacm cache verify | Rehash & integrity check | Optional |

## Performance Strategies
- Streaming: never store full tarball in memory, hash while streaming
- Zero-copy: reuse shared buffers where possible (ring buffers)
- Parallelism: separate resolver (CPU) and fetch (I/O) task groups
- Hash Acceleration: leverage `std.crypto.hash.sha2.Sha512` incremental API
- Minimal Syscalls: batch filesystem operations (deferred directory creation)
- Warm Cache: persistent metadata index (LMDB / simple binary map later)

## Security Considerations
| Risk | Mitigation |
|------|------------|
| Path Traversal in tar | Reject entries with '..' / absolute root |
| Malicious postinstall | Script sandbox, allow/deny list (future) |
| Integrity mismatch | Abort & purge partially written store entry |
| Race writing store | Lock file, atomic directory rename |
| Symlink attacks | Validate target inside project or store |

## Implementation Notes (Rust)
- Memory: GeneralPurposeAllocator for now; move to custom arena pools per install session.
- Error Handling: bubble with `try`; central command dispatcher reports and exits.
- Concurrency (future): async/await tasks for fetch + extraction; use bounded channel.
- Hashing: std.crypto.sha2 (streaming) -> hex / base64 encoding for integrity string.
- Platform: Windows symlink policy -> use junctions for directories when necessary.

## Contributing (Early Prototype)
- Expect rapid schema changes.
- Keep functions small & explicit about allocation ownership.
- Prefer adding unit tests for parser / resolver pieces as they land.

## Quick Start (Prototype)
```
cargo build --release
target\release\pacm   # help
target\release\pacm init
target\release\pacm install lodash   # placeholder (no network yet)
```

## Future Compatibility Goals
- Respect NODE_ENV, workspace root detection.
- Interop with existing projects (read existing package.json, produce compatible node_modules).
- Optionally export pnpm-style lockfile for ecosystem tools.

---
Status: PROTOTYPE – Not production ready.
