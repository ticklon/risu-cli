# Risu Note CLI 🐿️

**Risu Note** is a local-first, terminal-based note-taking application designed for developers. It features Vim-like keybindings, robust offline capabilities, and optional End-to-End (E2E) encrypted synchronization.

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Build Status](https://github.com/ticklon/risu-cli/actions/workflows/ci.yml/badge.svg)

[English](README.md) | [日本語](README_JP.md)

## ✨ Features

- **vim-like Navigation:** Navigate and edit notes without leaving the keyboard.
- **Local-First:** All data is stored locally in SQLite (`~/.risu/local.db`). Works perfectly offline.
- **E2E Encryption:** Sync uses Argon2id for key derivation and ChaCha20Poly1305 for encryption. The server *never* sees your plain text.
- **Secure Architecture:** Authentication tokens and passphrases are strictly managed (local file with 600 permissions).
- **Cross-Platform:** Runs on macOS, Linux, and Windows.

## 🚀 Installation

### From Crates.io

The easiest way to install Risu is via [crates.io](https://crates.io/crates/risu):

```bash
cargo install risu
```

### From Source

Ensure you have [Rust](https://www.rust-lang.org/tools/install) installed.

```bash
git clone https://github.com/ticklon/risu-cli.git
cd risu-cli
cargo install --path .
```

## 📖 Usage

Run the application:

```bash
risu
```

### Key Bindings (Basic)

- `j` / `k` (or Up/Down): Navigate list
- `Enter`: Open note in Editor (Normal Mode)
- `i`: Open note in Editor (Insert Mode)
- `n`: Create new note (starts in Insert Mode)
- `d`: Delete note (with confirmation)
- `/`: Search / Filter notes
- `Ctrl+g`: Show Status Pane (from List Mode)
- `Esc`: Back to List (Auto-saves changes)
- `Ctrl+s`: Force Save / Sync 

## 🔐 Security & Privacy

Risu Note prioritizes your privacy.
- **No Plaintext Sync:** Data is encrypted on your device before it touches the network.
- **Zero Knowledge:** We cannot recover your data if you lose your passphrase.

See our [Privacy Policy](docs/legal/privacy_policy.md) and [Terms of Service](docs/legal/terms_of_service.md) for details.

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## 📄 License

This project is licensed under the [MIT License](LICENSE).
