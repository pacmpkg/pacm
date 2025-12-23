<div align="center">

<img src="assets/avatar-no-bg.png" alt="Pacm Logo" width="248" height="248"/>

*Fast, disk-efficient, secure JavaScript/TypeScript package manager (prototype)*

[![License: ICL-1.0](https://img.shields.io/badge/License-ICL--1.0-blue?style=for-the-badge&label=License&labelColor=000000&color=00BDFD)](https://github.com/pacmpkg/pacm/blob/main/LICENSE)
[![Version](https://img.shields.io/github/v/release/pacmpkg/pacm?include_prereleases&sort=date&display_name=release&style=for-the-badge&label=Version&labelColor=000000&color=00BDFD)](https://github.com/pacmpkg/pacm/releases/latest)
<br />
[![By InfiniteJS](https://img.shields.io/badge/by-infinitejs-blue.svg?style=for-the-badge&label=By&labelColor=000000&color=000000&logo=data:image/svg+xml;base64,PHN2ZyByb2xlPSJpbWciIHZpZXdCb3g9IjAgMCAyNCAyNCIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj48dGl0bGU+SW5maW5pdGVKUzwvdGl0bGU+PHBhdGggdHJhbnNmb3JtPSJzY2FsZSgwLjAyMzQzNzUpIHRyYW5zbGF0ZSgwLjAxMTcxODc1LDAuMDExNzE4NzUpIiBmaWxsPSIjZmZmZmZmIiBkPSJNIDEwMjMuNSw0ODAuNSBDIDEwMjMuNSw0OTYuODMzIDEwMjMuNSw1MTMuMTY3IDEwMjMuNSw1MjkuNUMgMTAxMS44LDYyMC4yMjMgOTY2LjEzLDY4OC4yMjMgODg2LjUsNzMzLjVDIDgwNi4xNzUsNzcyLjI5MyA3MjUuNTA4LDc3Mi4xMjYgNjQ0LjUsNzMzQyA2MTguMzY3LDcxNy45OTYgNTk0Ljg2Nyw2OTkuNDk2IDU3NCw2NzcuNUMgNDk5LjQ5LDU5MS4xMjYgNDI0Ljk5LDUwNC43OTMgMzUwLjUsNDE4LjVDIDMxNC4wOTQsMzgyLjM5MyAyNzAuNzYsMzcwLjU1OSAyMjAuNSwzODNDIDE3Mi4xMTcsMzk4LjcyMyAxNDIuNjE3LDQzMS41NTcgMTMyLDQ4MS41QyAxMjUuMzY5LDUzMy42NDcgMTQyLjg2OSw1NzUuNDgxIDE4NC41LDYwN0MgMjE5LjA3NCw2MjguNjU0IDI1Ni4wNzQsNjMzLjk4NyAyOTUuNSw2MjNDIDMxNi45MzQsNjE1LjU0NiAzMzUuNDM0LDYwMy43MTIgMzUxLDU4Ny41QyAzNjIuNjU3LDU3My41MTEgMzc0LjY1Nyw1NTkuODQ1IDM4Nyw1NDYuNUMgMzkyLjAyNSw1NDAuMzA5IDM5Ni44NTgsNTMzLjk3NiA0MDEuNSw1MjcuNUMgNDAzLjU5OSw1MjUuNDggNDA1LjkzMyw1MjUuMTQ2IDQwOC41LDUyNi41QyA0MzUuNDY4LDU1OC45NjcgNDYyLjk2OCw1OTAuOTY3IDQ5MSw2MjIuNUMgNDkxLjY2Nyw2MjQuNSA0OTEuNjY3LDYyNi41IDQ5MSw2MjguNUMgNDc2LjMyNCw2NDUuMjA3IDQ2MS42NTcsNjYxLjg3NCA0NDcsNjc4LjVDIDM4MC42OTMsNzQ1Ljc0NSAzMDAuNTI3LDc3MS41NzkgMjA2LjUsNzU2QyAxMTguMzA0LDczNi43ODEgNTUuNDcxLDY4Ni4yODEgMTgsNjA0LjVDIDE0LjMzMzMsNTkzLjgzMyAxMC42NjY3LDU4My4xNjcgNyw1NzIuNUMgNC40NTk4Myw1NjAuMzYzIDEuOTU5ODMsNTQ4LjM2MyAtMC41LDUzNi41QyAtMC41LDUxNC4xNjcgLTAuNSw0OTEuODMzIC0wLjUsNDY5LjVDIDExLjY4NDYsMzk0LjIzMiA0OC4zNTEzLDMzNC4zOTggMTA5LjUsMjkwQyAxNzkuMDE5LDI0Ni4zMDMgMjUzLjY4NSwyMzUuMzAzIDMzMy41LDI1N0MgMzgxLjU1MywyNzIuMSA0MjIuMDUzLDI5OC42IDQ1NSwzMzYuNUMgNTI3LjA1Nyw0MjAuODc5IDU5OS4zOTEsNTA0Ljg3OSA2NzIsNTg4LjVDIDcxMS4zODEsNjI2LjgwNyA3NTcuNTQ4LDYzNy42NCA4MTAuNSw2MjFDIDg2NS40NTEsNTk2Ljk3MiA4OTIuNjE3LDU1NC44MDUgODkyLDQ5NC41QyA4ODYuNjAyLDQ0Ny4xNiA4NjMuMTAyLDQxMi4zMjcgODIxLjUsMzkwQyA3NTYuNjAyLDM2NS41OTMgNzAyLjQzNiwzNzkuNzYgNjU5LDQzMi41QyA2NDYuNTcxLDQ0Ny42ODUgNjMzLjkwNCw0NjIuNjg1IDYyMSw0NzcuNUMgNjE5LjQ0OCw0NzguOTY2IDYxNy42MTQsNDc5Ljk2NiA2MTUuNSw0ODAuNUMgNTkwLjgyNCw0NTEuOTkzIDU2NS45OTEsNDIzLjY2IDU0MSwzOTUuNUMgNTM4LjIyOSwzOTEuNjEgNTM1LjIyOSwzODcuOTQzIDUzMiwzODQuNUMgNTMxLjMzMywzODIuMTY3IDUzMS4zMzMsMzc5LjgzMyA1MzIsMzc3LjVDIDU1MC45NTMsMzU1LjE5NyA1NzAuNzg2LDMzMy4zNjQgNTkxLjUsMzEyQyA2NTAuMjc0LDI2MC43NzUgNzE4Ljk0MSwyMzkuNDQyIDc5Ny41LDI0OEMgODgzLjU5NywyNjAuNTc3IDk0OC40MywzMDQuMDc3IDk5MiwzNzguNUMgMTAwOC43Nyw0MTAuOTkgMTAxOS4yNyw0NDQuOTkgMTAyMy41LDQ4MC41IFoiPjwvcGF0aD48L3N2Zz4K)](https://github.com/infinitejs)

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
