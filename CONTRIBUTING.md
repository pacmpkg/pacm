# Contributing to Pacm

Thank you for your interest in contributing to pacm! We welcome contributions from everyone. This document provides guidelines and information for contributors.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Making Changes](#making-changes)
- [Testing](#testing)
- [Submitting Changes](#submitting-changes)
- [Reporting Issues](#reporting-issues)
- [Community](#community)

## Code of Conduct

This project follows a code of conduct to ensure a welcoming environment for all contributors. Please read and follow our [Code of Conduct](CODE_OF_CONDUCT.md).

## Getting Started

### Prerequisites

- Rust 1.70 or later
- Git
- Basic knowledge of Rust programming

### Development Setup

1. **Fork the repository** on GitHub
2. **Clone your fork**:
   ```bash
   git clone https://github.com/YOUR_USERNAME/pacm.git
   cd pacm
   ```
3. **Add upstream remote**:
   ```bash
   git remote add upstream https://github.com/infinitejs/pacm.git
   ```
4. **Create a branch** for your changes:
   ```bash
   git checkout -b feature/your-feature-name
   ```
5. **Build the project**:
   ```bash
   cargo build
   ```
6. **Run tests**:
   ```bash
   cargo test
   ```

## Making Changes

### Code Style

- Follow Rust's standard formatting. Run `cargo fmt` before committing
- Use `cargo clippy` to check for common mistakes and style issues
- Write clear, concise commit messages
- Add tests for new functionality
- Update documentation as needed

### Commit Guidelines

- Use clear, descriptive commit messages
- Start with a verb in imperative mood (e.g., "Add", "Fix", "Update")
- Keep commits focused on a single change
- Reference issue numbers when applicable (e.g., "Fix #123")

Example:
```
Add support for scoped npm packages

- Parse @scope/package format in package specs
- Update resolver to handle scoped packages
- Add tests for scoped package resolution
```

### Architecture Guidelines

- Keep the core library modular and well-documented
- Prefer functional programming patterns where appropriate
- Handle errors gracefully with proper error types
- Write performant code, but prioritize correctness first

## Testing

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test modules
cargo test --test lockfile
cargo test --test manifest

# Run with verbose output
cargo test -- --nocapture
```

### Adding Tests

- Add unit tests in the same file as the code being tested using `#[cfg(test)]` modules
- Add integration tests in the `tests/` directory
- Ensure tests are fast and reliable
- Use descriptive test names

### Test Coverage

Aim for good test coverage, especially for:
- Core functionality (resolver, lockfile, manifest)
- Error handling
- Edge cases

## Submitting Changes

### Pull Request Process

1. **Ensure your branch is up to date**:
   ```bash
   git fetch upstream
   git rebase upstream/main
   ```

2. **Run the full test suite**:
   ```bash
   cargo test
   cargo clippy
   cargo fmt --check
   ```

3. **Push your changes**:
   ```bash
   git push origin feature/your-feature-name
   ```

4. **Create a Pull Request** on GitHub:
   - Use a clear, descriptive title
   - Fill out the pull request template
   - Reference any related issues
   - Provide a summary of changes

### Pull Request Requirements

- All tests must pass
- Code must be formatted with `cargo fmt`
- No clippy warnings
- Documentation updated if needed
- Changes reviewed and approved

## Reporting Issues

### Bug Reports

When reporting bugs, please include:

- **Clear title** describing the issue
- **Steps to reproduce** the problem
- **Expected behavior** vs. actual behavior
- **Environment information** (OS, Rust version, etc.)
- **Error messages** or stack traces
- **Minimal reproduction case** if possible

### Feature Requests

For feature requests, please:

- **Check existing issues** to avoid duplicates
- **Describe the problem** you're trying to solve
- **Explain your proposed solution**
- **Consider alternative approaches**
- **Discuss potential impact** on users and codebase

## Community

- **Discussions**: Use GitHub Discussions for general questions
- **Issues**: Report bugs and request features via GitHub Issues
- **Pull Requests**: Submit code changes via Pull Requests
- **Discord/Slack**: Join our community chat

## Recognition

Contributors will be recognized in:
- Git commit history
- CHANGELOG.md (for significant changes)
- GitHub's contributor insights

Thank you for contributing to pacm! 🚀