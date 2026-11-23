# Changelog

[Keep a Changelog](https://keepachangelog.com/ja/1.1.0/) ベース。ワークスペース版で 4 クレートを同期更新する。実機検証の根拠は [`docs/dev/05-verification.md`](docs/dev/05-verification.md) のみ。

## [1.0.0] - 2026-11-23

初回公開リリース（`m241-core` / `m241-flash` / `m241-cli` / `m241-gui` を `1.0.0` に統一。非公開開発版 `0.1.0` からの昇格）。実機走行は `b8d91ee` 時点で、以降の flash / CLI 変更は実機未実行。

### Added

- **読み取り・変換基盤（`m241-core`）**: `$PMTK705` 識別、NMEA 三値判定＋`$PMTK180*3B` BootROM 進入、`0xA2` 分割吸い出し（2 回読み一致）、DA RAM ロード／`0xD6` 読み出し、PMTK182 ログ取得、ブロックヘッダ長（16/20 B）＋設定ジャーナル対応（PR #34）、GPX / KML / CSV / NMEA / JSON / HTML / RTF / KMZ 変換、トラック編集、自前 EXIF ジオタグ（時刻シフト・スコープ・距離グループ化）、WNRO パッチ計画（`0x402F4`、`315964800` → `935280000`、V1.13/V1.12 プロファイル）。
- **書き込み基盤（`m241-flash`）**: `DA_v3.8.4.bin` 逆解析ベースの `0xD3`/`0xD5`/`0xD9`、固定範囲（`0` / `0x6345C`）＋`check_layout`、`verify_device` と全長読み戻し、`--recover`、`--allow-vendor`（公式 V1.13 固定 SHA-256 ゲート）、`DaDigest::vendor()` ピン、`fw da-restart`、ログ消去（`BackupProof` 必須）。
- **CLI（`m241`、17 コマンド）**: `device`（`ports`/`info`/`doctor`）、`fw`（`dump`/`patch`/`flash`/`probe-da`/`da-restart`）、`log`（`info`/`download`/`read`/`project`/`edit`/`erase`/`geotag`/`kmz`）、`nmea`（`repair`/`proxy`）。`--json` と終了コード 0–6。
- **ezTour パリティ（PR #36 / #41）**: `--gpx`/`--kml`/`--kmz` 読込、`--cache`（警告リプレイ付き v2 キャッシュ）、`--csv-report`、`--html`、`--rtf`、`--coord-format`（`decimal`/`dms`/`utm`）、`--simplify`、`log info` 統計、`--group-photo-distance`、`--no-backup`、`--icons`。
- **GUI（`m241-gui`、PR #31 / #37 / #38 / #39 / #42）**: 4 画面（接続 / ログ / ファーム / 設定）。ファーム画面から 2 回読み吸い出し・パッチ・`WRITE` 確認書き込み・復旧・`PROBE`/`RESTART`、ログ画面に「ログ操作」コンソール（消去は拒否停止で CLI 専用）と OSM ラスタ地図（Web Mercator、30 日/200 MB キャッシュ、帰属表示、オフライン模式背景、ヘッドレス非通信）。
- **Tauri v2 GUI（`crates/m241-app`、PR #43）**: シェル・Connect・Log 画面（読み取り専用、MapLibre OSM 減彩表示、安全ゲート 3 条項追加）。
- **安全・配布**: `m241-core` / `m241-flash` 分離、`./tools/check.sh` 6 ゲート、`.github/workflows/release.yml`（`v1.0.0` タグ・Release は未作成）。

### Changed

- **EXIF 書き換えの `_original` バックアップを既定 ON に変更**（`GeotagOptions` 既定のため CLI・GUI 双方に適用。旧挙動は `--no-backup` / `keep_backup: false`）。
- `log read --format nmea`: 未測定の GGA 衛星数を `00` から空欄へ変更。
- `log read --format kml`: KML 2.0 `FeatureType` 順（`<LookAt>` を `<Style>` 前）へ修正。
- `log read --gpx`: 歴史的 GPX の誤シフト防止のため `--utc-offset-days auto` を無効化（警告表示、明示日数は適用）。

### Fixed

- `--cache` ヒット時の解析警告消失、`--gpx` 0 点時の exit 0 空出力、`log info` 0 fix 時の `distance: 0.00 m`（`-` に修正）、`--simplify 1` の誤報告、非有限座標の GPX/KML 出力を修正。

## 実機確認・未検証の要約

- **実機確認済み**（[`docs/dev/05-verification.md`](docs/dev/05-verification.md) §1.5 / §1a）: `fw dump`（2 回一致）、`fw probe-da`（DA 基底 `0`）、`fw patch`、`fw flash --recover` と通常書き込み（1589 パケット完走・406,620 B 読み戻し一致・V1.14 起動）、`fw da-restart`、`log erase`（2026-11-21）、`nmea proxy`（stdout / loopback TCP 9 文）、GUI 接続画面「接続して確認」。`fw flash --allow-vendor` は完走・起動するが、ファームが個体データ `0x5C`–`0x5F`（全 14 B の一部）を復元するため `0x5C` で読み戻し不一致。
- **主な未検証**（同 §2）: 測位ありの日付、書き込み後のログ記録、OLED 表示、GUI からの実機書き込み・吸い出し・中継・追従・JPEG 書き込み、Wayland 実画面地図見た目、実カメラ JPEG・Google Earth/`unzip`、macOS、Bluetooth SPP、V1.11/V1.12/4 MiB/GR-245、`0xD3` 自動リトライ、`0xD6` 複数ページ。

## 注意

ファームウェア書き換えは端末を恒久的に破損しうる（JTAG なし）。実行前に [`DISCLAIMER.md`](DISCLAIMER.md) を読み、必ず自機バックアップを保管すること。コード・文書は MIT（[`LICENSE`](LICENSE)）、ベンダーバイナリは非同梱。

[1.0.0]: https://github.com/haruki0420/holux-m241-rs/releases/tag/v1.0.0
