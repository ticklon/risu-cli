# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.13] - 2026-01-24

### Fixed
- **Sync Critical Bug**: Fixed an issue where initial sync would skip encrypted notes if the key wasn't available yet, permanently advancing the sync cursor and causing data loss in the UI.
- **Decryption Recovery**: Notes that fail to decrypt (due to key mismatch or corruption) are no longer skipped. They are now saved with a placeholder error message, preventing the sync process from getting stuck.
- **Cross-Platform Compatibility**: Added auto-detection and recovery for encrypted notes that were incorrectly flagged as plaintext by older Windows clients.
- **CLI Login**: Fixed a bug where `risu login` via CLI did not sync the encryption salt, causing subsequent TUI sessions to fail decryption.
- **UI State**: Fixed an issue where the status indicator incorrectly showed "Offline" after clearing all data.

### Added
- **Reset Command**: Added `risu reset-local` command to forcefully clear the local database and reset the sync state, useful for recovering from sync issues.

### Documentation
- **Enhanced Description**: Updated README and Cargo.toml to better reflect the project's focus on speed, local-first design, and optional E2E sync.
- **Prerequisites**: Added Nerd Fonts recommendation for optimal UI experience.

## [0.1.12] - 2026-01-23

### Fixed
- **UI Layout**: Optimized Status Pane layout to prevent menu truncation on smaller terminal screens.
- **Sync Status**: Fixed an issue where the sync status indicator remained "Synced" immediately after logging out.
- **Display**: Corrected plan display names and menu item labels for better clarity.

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
