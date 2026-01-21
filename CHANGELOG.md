# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-01-21

### Added
- **CLI Commands**: Restored `risu login` and `risu logout` commands for easier authentication without entering the TUI.
- **Login UX**: Added a spinner animation and suppressed verbose error logs during login polling.
- **Already Logged-in Check**: Added detection and informative messaging when `login` or `logout` is called in an already authenticated/unauthenticated state.

### Changed
- **CLI Parsing**: Migrated argument parsing to `clap` for better developer experience and standard help messaging.

### Removed
- **E2E CLI Commands**: Removed unused and deprecated `e2e` subcommand logic to focus on TUI-driven setup.

## [0.1.0] - 2026-01-21

### Added
- Initial public release on GitHub and crates.io.
- **Core Features**:
    - Local-first storage using SQLite (`~/.risu/local.db`).
    - Vim-like keybindings (j/k navigation, Normal/Insert modes).
    - Basic Markdown preview with `m` key.
    - End-to-End (E2E) Encrypted Synchronization using Argon2id and ChaCha20Poly1305.
    - Security focused: 600 permissions for local keys and structured logging.
