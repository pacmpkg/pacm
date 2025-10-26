# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial prototype implementation
- Lockfile management with binary format
- Package manifest handling
- Dependency resolution engine
- CLI interface with basic commands
- Comprehensive testing suite
- GitHub Actions CI/CD workflows
- Issue and PR templates
- Security policy and code of conduct
- Contributing guidelines

### Changed
- Consolidated tests into integration test suite

### Technical
- Built with Rust for performance and safety
- Modular architecture with separate crates for components
- Cross-platform support (Windows, macOS, Linux)

## [0.1.0] - 2025-10-26

### Added
- Initial release
- Basic package management functionality
- Lockfile format v1 and v2 support
- NPM-style semver range resolution
- Cache management
- CLI commands: init, install, add, remove, list, cache

### Known Issues
- Prototype implementation - not production ready
- Limited package registry support
- No advanced features like workspaces or monorepos