# Holux M-241 の日付が 19.6 年ずれる問題を直す（holux-m241-rs）

Holux M-241（MediaTek MT3318）の 2006 年製ファームウェアは GPS エポック定数 `315964800` を直書きしており、2019-04-06 のロールオーバーで日時が 7,168 日（19.6 年）過去になる。本ツールは `0x402F4` の定数を `935280000` へ書き換え（変化は 3 バイト）、軌跡ログの吸い出し・変換・地図表示・写真ジオタグを行う。

## 現在の状態（1.0.0）

- **実機で確認済み**（[`docs/dev/05-verification.md`](docs/dev/05-verification.md) §1.5 / §1a）:
  - 読み取り系（`device info` / `fw dump` / `fw probe-da` / `log download`）、`fw patch`、`fw flash`（復旧・通常書き込みとも 1589 パケット完走・406,620 B 読み戻し一致・V1.14 起動確認。ただし実機走行は `b8d91ee` 時点で、以降の DA ダイジェストゲート等は実機未実行）。
  - `log erase`（2026-11-21 確認、後続変更は未検証）、`nmea proxy`（stdout およびループバック TCP 9 文確認）、GUI 接続画面「接続して確認」（読み取り専用）、GUI ログ画面の実ログ＋OSM タイル表示（開発機 X11、実機非接続）。
  - ログ解析は PR #34 で設定ジャーナル誤読を修正済み（保存済みダンプの再解析で確認、実機未走行）。
- **未検証**: 測位ありの日付、書き込み後のログ記録、GUI からの実機書き込み／吸い出し／中継／追従描画／JPEG 書き込み、Wayland 実画面の地図見た目、`nmea proxy` 長時間・複数クライアント・非ループバック、`device doctor` 実機 probe、実カメラ JPEG ジオタグ、macOS、Bluetooth SPP、他モデル。ベンダー公式 V1.13 書き込みは完走・起動するが、個体データ 14 バイト（`0x5C`–`0x5F` 4 B、`0xF8`–`0xFD` 6 B、`0x63458`–`0x6345B` 4 B）のうち `0x5C`–`0x5F` を起動時にファームが復元するため、実測では `0x5C` の 1 バイト差で読み戻し照合が一致しない（[`docs/dev/06-open-questions.md`](docs/dev/06-open-questions.md) §8）。一覧は [`docs/dev/05-verification.md`](docs/dev/05-verification.md) §2 を参照。

## 主な機能

| 用途 | コマンド / 画面 |
|---|---|
| ポート探索・識別・診断 | `m241 device ports` / `info` / `doctor` |
| ファーム吸い出し（安全）・パッチ（PC 内） | `m241 fw dump` / `patch` |
| ファーム書き込み（破壊的）・DA 確認・再起動 | `m241 fw flash` / `probe-da` / `da-restart` |
| ログ情報・吸い出し・変換・編集・消去 | `m241 log info` / `download` / `read` / `project` / `edit` / `erase` |
| 写真ジオタグ・KMZ 生成 | `m241 log geotag` / `kmz` |
| NMEA 日付修復・ライブ中継 | `m241 nmea repair` / `proxy` |
| デスクトップ GUI（4 画面: 接続/ログ/ファーム/設定） | `m241-gui`（CLI と同じ処理・OSM 地図・`WRITE` 手入力） |

## ネットワークとプライバシー

- **既定 ON の外部通信は GUI / Tauri 版ログ画面の OpenStreetMap タイル取得のみ**（表示範囲が OSM に伝わるため、滞在場所の推測を避けたい場合は設定画面またはツールバーで OFF にする。OFF / オフライン時は模式背景で軌跡・再生・書き出しを継続可）。
- egui 版は OS キャッシュに保存（30 日 TTL、200 MB 上限、設定で消去可）、タイル描画フレームに `© OpenStreetMap contributors` を表示。`--screenshot` と headless 描画は通信しない。Tauri 版は WebView キャッシュを使用。
- CLI はインターネットに接続しない（`nmea proxy --bind` は既定で `127.0.0.1` のみ）。

## インストール

### リリースバイナリ

安定版タグ（`v1.0.0` 等）の push で GitHub Release に Linux / Windows / macOS バイナリ、`LICENSE`、`SHA256SUMS.txt` が公開される予定（**`v1.0.0` タグ・Release は未作成**）。

| OS | CLI | GUI |
|---|---|---|
| Linux (x86_64, Ubuntu 24.04 / glibc 2.39 + `libudev.so.1`) | `m241-linux-x86_64` | `m241-gui-linux-x86_64` |
| Windows (x86_64) | `m241-windows-x86_64.exe` | `m241-gui-windows-x86_64.exe` |
| macOS (aarch64) | `m241-macos-aarch64` | `m241-gui-macos-aarch64` |

`SHA256SUMS.txt` で照合し、`PATH` に配置する（未署名バイナリのため macOS は Gatekeeper 解除、`xattr -d com.apple.quarantine`、Windows は SmartScreen の「詳細情報 > 実行」が必要な場合あり。Linux は `dialout` グループ所属が必要）。詳細は [`docs/user/01-install.md`](docs/user/01-install.md)（英語）。

### ソースからビルド

要件: Rust 1.95+。Tauri 版（`crates/m241-app`）には Node.js と **pnpm**（npm/npx 不可）が必要。Linux では `libudev-dev libxkbcommon-dev libwayland-dev libx11-dev libxi-dev libgl1-mesa-dev libegl1-mesa-dev pkg-config` を導入する。

```bash
pnpm --dir crates/m241-app/ui install --frozen-lockfile && pnpm --dir crates/m241-app/ui build
cargo build --release     # target/release/m241, target/release/m241-gui
./tools/check.sh          # fmt / clippy / test / docs / design / safety（ui/dist 不在時は pnpm を自動実行）
```

### 使い方（例）

```bash
m241 device ports && m241 device info
m241 fw dump -o backup/firmware.bin                           # 2 回読み＋電源入れ直し
m241 fw patch backup/firmware.bin -o backup/firmware-v114.bin # PC 内で完結
m241 log download -o backup/log.bin
m241 log read backup/log.bin --format gpx -o track.gpx
```

書き込み（`m241 fw flash`）は破壊的。必ず [`DISCLAIMER.md`](DISCLAIMER.md) と [`docs/user/02-fix-the-date.md`](docs/user/02-fix-the-date.md)（英語）を読み、自機バックアップを用意してから行う。失敗時は [`docs/user/03-recovery.md`](docs/user/03-recovery.md)（英語）を参照。

## 含まれないもの・設計方針

- Holux ファームウェア、`DA_v3.8.4.bin`、ベンダー DLL/EXE は再配布しない（書き込み時のみローカルから読み込む）。
- DA ドライバ（`crates/m241-flash/src/da.rs`）は既存 Python 実装を移植せず `DA_v3.8.4.bin` 逆解析（[`docs/reference/01-da-protocol.md`](docs/reference/01-da-protocol.md)）から再構築し、読み取り専用 `m241-core` と分離している。経緯は [`docs/dev/04-prior-art.md`](docs/dev/04-prior-art.md) 参照。

## ドキュメント

| 区分 | ページ | 内容 |
|---|---|---|
| 利用者 | [`CHANGELOG.md`](CHANGELOG.md) / [`DISCLAIMER.md`](DISCLAIMER.md) / [`PRODUCT.md`](PRODUCT.md) | 変更履歴・免責・製品方針 |
| 利用者 | [`docs/user/01-install.md`](docs/user/01-install.md) / [`02-fix-the-date.md`](docs/user/02-fix-the-date.md) / [`03-recovery.md`](docs/user/03-recovery.md) | 導入・日付修正・復旧手順（英語） |
| 参照 | [`docs/reference/02-cli.md`](docs/reference/02-cli.md) / [`01-da-protocol.md`](docs/reference/01-da-protocol.md) | CLI 仕様・DA プロトコル逆解析 |
| 開発者 | [`docs/dev/03-hardware-facts.md`](docs/dev/03-hardware-facts.md) / [`05-verification.md`](docs/dev/05-verification.md) / [`06-open-questions.md`](docs/dev/06-open-questions.md) | 実測定数・検証台帳・未解決事項 |
| 開発者 | [`docs/dev/07-eztour-spec.ja.md`](docs/dev/07-eztour-spec.ja.md) / [`11-safety-review.md`](docs/dev/11-safety-review.md) / [`DESIGN.md`](DESIGN.md) / [`AGENTS.md`](AGENTS.md) | ezTour 仕様・安全レビュー・設計・作業規則 |

## ライセンス

コード・ドキュメントは MIT（[`LICENSE`](LICENSE)）。プロプライエタリなバイナリの権利は各所有者に帰属する。
