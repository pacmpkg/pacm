<div align="center">

<img src="assets/avatar-no-bg.png" alt="Pacm Logo" width="248" height="248"/>

*Fast, disk-efficient, secure JavaScript/TypeScript package manager (prototype)*

[![License: ICL-1.0](https://img.shields.io/badge/License-ICL--1.0-blue.svg)](https://github.com/pacmpkg/pacm/blob/main/LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange)](https://www.rust-lang.org/)
[![CI](https://github.com/pacmpkg/pacm/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/pacmpkg/pacm/actions/workflows/ci.yml)

</div>

## 🚀 About

Pacm is a blazing-fast, cache-first package manager for JavaScript and TypeScript projects. Built with Rust for maximum performance and reliability, it aims to provide a secure and efficient alternative to existing package managers.

## ✨ Features

- **Lightning Fast**: Written in Rust for optimal performance
- **Cache-First**: Intelligent caching reduces installation times
- **Secure**: Cryptographic integrity verification for all packages
- **Disk Efficient**: Minimal disk usage through deduplication
- **NPM Compatible**: Works with existing npm packages and package.json files
- **Cross-Platform**: Supports Windows, macOS, and Linux

## 📦 Installation

### From Source

```bash
git clone https://github.com/pacmpkg/pacm.git
cd pacm
cargo build --release
# Binary will be available at target/release/pacm
```

### Pre-built Binaries

*Coming soon - check releases for pre-built binaries*

## 🛠️ Usage

### Initialize a new project

```bash
pacm init --name my-project
```

### Install dependencies

```bash
pacm install
# or
pacm i
```

### Add a package

```bash
pacm add lodash
pacm add axios --dev
```

### Remove a package

```bash
pacm remove lodash
```

### List installed packages

```bash
pacm list
```

### Cache management

```bash
pacm cache path    # Show cache location
pacm cache clean   # Clear cache
```

### Advanced commands

```bash
pacm pm lockfile   # Manage lockfile
pacm pm prune      # Remove unused packages
```

## 🏗️ Architecture

Pacm is built with a modular architecture:

- **Core Library** (`src/`): Lockfile management, manifest handling, dependency resolution
- **CLI** (`src/cli/`): Command-line interface and commands
- **Cache** (`src/cache/`): Package caching and retrieval
- **Fetcher** (`src/fetch/`): Package downloading and verification
- **Installer** (`src/installer/`): Package installation logic
- **Resolver** (`src/resolver/`): Dependency resolution algorithms

## 🧪 Testing

The project includes a comprehensive testing suite located in `tests/`. Run tests with:

```bash
cargo test
```

See [tests/README.md](https://github.com/pacmpkg/tests/blob/main/README.md) for details about the testing structure.

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

### Development Setup

```bash
git clone https://github.com/pacmpkg/pacm.git
cd pacm
cargo build
cargo test
```

### Code Style

This project follows Rust's standard formatting. Please run `cargo fmt` before submitting PRs.

## 🤝 Contributing

We welcome contributions! Please see our [Contributing Guide](CONTRIBUTING.md) for details.

### Community Guidelines

- [Code of Conduct](CODE_OF_CONDUCT.md) - Our community standards
- [Security Policy](SECURITY.md) - Reporting security vulnerabilities
- [Issue Templates](.github/ISSUE_TEMPLATE/) - How to report bugs and request features

## 📄 License

Licensed under ICL-1.0.

## ⚠️ Disclaimer

This is a **prototype** implementation. It is not yet ready for production use. Use at your own risk.

## 📞 Contact

- Repository: https://github.com/pacmpkg/pacm
- Issues: https://github.com/pacmpkg/pacm/issues
- Discussions: https://github.com/pacmpkg/pacm/discussions
