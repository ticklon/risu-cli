# Risu Note CLI 🐿️

**Risu Note** は、開発者のために設計された Local-First なターミナル製ノートアプリです。Vim ライクなキーバインディング、強力なオフライン機能、そしてオプションのエンドツーエンド (E2E) 暗号化同期機能を備えています。

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Build Status](https://github.com/ticklon/risu-cli/actions/workflows/ci.yml/badge.svg)

[English](README.md) | [日本語](README_JP.md)

## ✨ 特徴 (Features)

- **Vim ライクな操作:** キーボードから手を離すことなく、ノートの閲覧や編集が可能です。
- **Local-First:** すべてのデータはローカルの SQLite (`~/.risu/local.db`) に保存されます。オフラインでも完璧に動作します。
- **E2E 暗号化:** 同期機能には Argon2id (鍵導出) と ChaCha20Poly1305 (暗号化) を採用しています。サーバーがあなたの平文データを見ることは*決して*ありません。
- **セキュアなアーキテクチャ:** 認証トークンやパスフレーズは厳重に管理されます (権限 600 のローカルファイル)。
- **クロスプラットフォーム:** macOS, Linux, Windows で動作します。

## 🚀 インストール (Installation)

### Crates.io から

最も簡単な方法は [crates.io](https://crates.io/crates/risu) 経由でのインストールです:

```bash
cargo install risu
```

### ソースコードから

[Rust](https://www.rust-lang.org/tools/install) がインストールされていることを確認してください。

```bash
git clone https://github.com/ticklon/risu-cli.git
cd risu-cli
cargo install --path .
```

## 📖 使い方 (Usage)

アプリを起動します:

```bash
risu
```

### 基本キーバインディング

- `j` / `k` (または Up/Down): リスト移動
- `Enter`: ノートをエディタで開く (ノーマルモード)
- `i`: ノートをエディタで開く (インサートモード)
- `n`: 新規ノート作成 (インサートモードで開始)
- `d`: ノート削除 (確認あり)
- `/`: 検索 / フィルタリング
- `Ctrl+g`: ステータス画面の表示 (リストモード時)
- `Esc`: リストに戻る (変更は自動保存されます)
- `Ctrl+s`: 強制保存 / 同期

## 🔐 セキュリティとプライバシー

Risu Note はあなたのプライバシーを最優先します。
- **平文同期なし:** データはネットワークに送信される前にデバイス上で暗号化されます。
- **ゼロ知識 (Zero Knowledge):** パスフレーズを紛失した場合、運営側でもデータを復元することはできません。

詳細は [プライバシーポリシー](docs/legal/privacy_policy.md) および [利用規約](docs/legal/terms_of_service.md) をご覧ください。

## 🤝 コントリビューション

貢献 (Contribution) は大歓迎です！お気軽に Pull Request を送ってください。

## 📄 ライセンス

このプロジェクトは [MIT ライセンス](LICENSE) の下で公開されています。
