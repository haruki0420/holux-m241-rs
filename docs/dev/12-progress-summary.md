# 進捗サマリ — holux-m241-rs

最終更新: 2026-11-23 JST

## 0. まず読む一次資料

1. [`AGENTS.md`](../../AGENTS.md)（§2 安全規則・§6 個人情報・§8 三段構成・§9 失敗記録）
2. [`PRODUCT.md`](../../PRODUCT.md) / [`DESIGN.md`](../../DESIGN.md)（製品方針・画面設計）
3. [`docs/dev/07-eztour-spec.ja.md`](07-eztour-spec.ja.md)（公式 ezTour v3.0 クリーンルーム仕様）
4. [`docs/dev/05-verification.md`](05-verification.md)（実機検証・未検証の唯一の台帳）
5. [`docs/reference/01-da-protocol.md`](../reference/01-da-protocol.md) / [`docs/dev/11-safety-review.md`](11-safety-review.md)（書き込み経路・分離）
6. [`docs/dev/04-prior-art.md`](04-prior-art.md) / [`docs/dev/06-open-questions.md`](06-open-questions.md)（先行実装差分・未解決課題）

## 1. 2 本の柱

Holux M-241（MT3318）向けクロスプラットフォーム Rust ツール:
1. **WNRO の安全な修復**: `0x402F4` の定数 `315964800` → `935280000`（3 バイト変更）。DA 処理は `DA_v3.8.4.bin` 逆解析から再構築。
2. **HOLUX ezTour v3.0 のクリーンルーム再実装**: 停止した Google Maps API・認証サーバ・外部 `exiftool` を排し、ログ取得・地図・変換・自前 EXIF・編集を再構築。

## 2. 現在の到達点（1.0.0）

- **構成・テスト・ゲート**: ワークスペース `1.0.0`（`m241-core` / `m241-flash` / `m241-cli` / `m241-gui` ＋ 新 Tauri 版 `m241-app`）。`./tools/check.sh` 6/6 PASS（`M241_VENDOR_IMAGE` 設定時は実 DA テストも実行、CI は skip）。`.github/workflows/release.yml` 整備済み（`v1.0.0` タグ・Release は未作成）。
- **統合済み**（[`10-review-log.md`](10-review-log.md) 記録済み）:
  - DA 逆解析（`01-da-protocol.md`）、ezTour 仕様化（`07-eztour-spec.ja.md`）、読み書き分離＋`tools/check_safety.py`。
  - コア読み取り・変換（GPX/KML/CSV/NMEA/JSON/HTML/RTF/KMZ）・設定ジャーナル対応（PR #34）・自前 EXIF ジオタグ（シフト・スコープ・距離グループ化）・トラック編集。
  - CLI 全 17 コマンド（スタブなし）。
  - egui GUI 4 画面（接続・ログ・ファーム・設定。ファーム書き込み・ログ操作コンソール・OSM 地図タイル実装済み）。
  - Tauri v2 GUI `crates/m241-app` P0〜P2（シェル・Connect・Log 読み取り専用、PR #43）。

## 3. 残っている課題

### 3.1〜3.4 ファーム書き込み・ezTour 機能・実機検証・GUI の残件

- **DA 書き込み・CLI**: 実機で `fw dump` / `fw probe-da`（基底 `0` 確定）/ `fw patch` / `fw flash`（通常・`--recover`、1589 パケット完走・406,620 B 読み戻し一致・V1.14 起動）/ `fw da-restart` / `log erase` / `nmea proxy`（stdout・loopback TCP）を確認済み（[`05-verification.md`](05-verification.md) §1.5 / §1a）。`--allow-vendor` は完走・起動するものの、ファームが個体データ `0x5C`–`0x5F` を復元するため読み戻し不一致（選択肢は [`06-open-questions.md`](06-open-questions.md) §8）。`--da` は `DaDigest::vendor()` で全文 SHA-256 固定。
- **ezTour P0〜P2**: コア・CLI・egui GUI に主要機能を実装済み。未確定・未検証: 実カメラ JPEG と IFD0 配置（`byCamera` 規則）、CSV `LOCAL DATE/TIME`（空出力）、KMZ の `icon/` 仕様と Google Earth/`unzip` 開封、非 JPEG の紐づけのみ。
- **GUI（PR #37/#38/#39）**: Connect 画面「接続して確認」は実機成功（2026-11-22）、Log＋OSM 地図は開発機 X11/Wayland で確認（2026-11-23、M-241 非接続）。**実機未検証**: GUI からのファーム書き込み・ログ吸い出し・中継・追従描画・JPEG 書き込み・KMZ/シフト/編集 UI、Wayland 実画面地図見た目、測位あり日付、書き込み後ログ記録、macOS、Bluetooth SPP、他モデル（V1.12・4 MiB・GR-245）。

### 3.5 TODO ロードマップ — 新 Tauri アプリ（`m241-app`）移植と ezTour 未実装機能

2026-11-23 に PR #43（`6b45d54`）で新 Tauri v2 + React アプリ（`crates/m241-app`）の P0〜P2（シェル・Connect 画面・Log 画面、読み取り専用）を `main` へ統合した。以下は今後実装する機能の TODO 一覧。

#### A. 新 Tauri アプリ（`crates/m241-app`）への既存機能移植

- [ ] **P3: ファームウェア画面・書き込み機能**（安全-critical のため 3 分割で実装）
  - [ ] **PR-P3a（Tier 2）**: `APP_MAY_LINK_FLASH = False` のまま、オフライン Patch（`fw_plan` / `fw_patch` / `fw_sample_plan`）と 3 ステップ骨格 UI
  - [ ] **PR-P3b（Tier 3）**: `APP_MAY_LINK_FLASH = True`・`tools/check_safety.py` の `check_app_write_route` / `check_app_port_path` 更新（`safety_gate.rs` 変異テスト同時追加）・Rust 側コマンド（IPC で生の確認語 `"WRITE"` / `"PROBE"` / `"RESTART"` を受け取り Rust 側で検証して `Confirmation::granted()` を構築、`tauri::ipc::Channel` で進捗通知）・`FakeDa` シミュレータテスト
  - [ ] **PR-P3c（Tier 3）**: Step 1（2 回読み吸い出し）・Step 3（通常書き込み / `--recover` / `--allow-vendor`）・琥珀色復旧パネル（`0xD9` 再起動・BootROM 再入・復旧）の UI 配線
- [ ] **P4: データ取り出し・ジオタグ書き込み・トラック編集**
  - [ ] デバイスからのログ吸い出し（`log download` 相当）とログ消去の安全案内
  - [ ] 写真ジオタグ EXIF 書き込み（時刻シフト適用・適用範囲 `all` / `selected` / `by-camera`・`_original` バックアップ・確認ダイアログ）
  - [ ] トラック編集 UI（分割・結合・点削除・Undo/Redo）
- [ ] **P5: 設定画面・周辺機能・磨き込み**
  - [ ] 設定画面（タイル ON/OFF・キャッシュ管理、座標形式切替、表示タイムゾーン、単位切替）
  - [ ] NMEA 修復・NMEA TCP 中継・プロジェクト保存/読込・`device doctor`
  - [ ] OS ファイルダイアログ（`tauri-plugin-dialog`、Log・Firmware の両画面で共用）
  - [ ] PR #43 見送り Should 項目（`useLogDocument` リクエストチケット S-4、`logdoc.rs` 責務分割 S-2、ESLint 導入 S-5、1024×640 レスポンシブ、タイル取得失敗案内、写真サムネイル帯・回転、KMZ 写真タイムゾーン反映、Tauri 実ウィンドウでの CSP・IME 検証）

#### B. ezTour 本来の機能のうちコア・CLI・GUI いずれにも未実装のバックログ

[`07-eztour-spec.ja.md`](07-eztour-spec.ja.md) および公式マニュアルとの突き合わせで確認した未実装項目。

- [ ] **端末のログ記録設定（`Config GPS…` / PMTK182 設定書き込み）**（要実機検証）
  - 時間間隔（`Log every N seconds`）・距離間隔（`Log every N meters`）
  - ログモードプリセット（General / Vehicle / Cycling / Hiking）
  - メモリ満杯時の挙動（`Overwrite` 上書き / `Stop Log` 停止）、デバイスリセット、単位（km / mile）
- [ ] **POI（地点メモ）の編集と手動追加**
  - 端末 POI ボタン地点の名前・メモ編集、地図クリックによる手動 POI 追加、種別アイコン選択
- [ ] **複数トラック管理**
  - 吸い出し時の取り込むトラック選択、複数トラック同時表示（10 色パレット）、トラックごとの色・線幅カスタマイズ
- [ ] **写真・メディア体験の拡充**
  - トラック再生と連動した写真スライドショー、写真コメント編集・回転表示
  - 非 JPEG（動画 MP4 等）のトラック紐づけのみ（EXIF 非書込）
- [ ] **トラック編集の地図上操作**
  - Shift 範囲選択・Ctrl トグル・矩形選択による点選択
- [ ] **地図ツール**
  - 地図種別切替（標準 / 衛星 / 地形 / 地理院地図等）、ミニマップ、地名検索、地図スクリーンショット保存
- [ ] **フォーマット互換・書き出しの細部**
  - GPX 点間引き（簡略化）オプション、GPX `<wpt>` 読み込み
  - CSV の `LOCAL DATE` / `LOCAL TIME` 出力（表示タイムゾーン設定と連動）
  - `kmzMaxPhotoEdgeSize` / `icon/` サムネイル寸法の GUI 指定経路
- [ ] **接続経路の実機検証**
  - Bluetooth SPP 経由の接続・読み出し（実機未検証）

※ 意図的に対象外とした項目（228 データム・14 各国グリッド、`.itm` / `.itx` / `.dst` / Yupiteru 形式の直接互換、Google Maps JS API v2/v3・認証サーバ・SNS アップロード・AGPS）は [`07-eztour-spec.ja.md`](07-eztour-spec.ja.md) §7–8 の方針を維持する。

## 4. 規約とコマンド早見表

規約: 三段構成（`AGENTS.md` §8）、安全ゲート維持（§2）、シミュレータ≠実機証明（`05-verification.md` §3）、個人情報・ベンダーバイナリ禁止（§6）、終了ワークツリー整理（§9.6）。

```bash
./tools/check.sh              # 6 ゲート（fmt / clippy / test / docs / design / safety）
cargo test --workspace
cargo build --release
cargo run -q -p m241-gui -- --screenshot
```
