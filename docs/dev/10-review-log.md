# レビュー記録

[`09-review-protocol.md`](09-review-protocol.md) に沿ったレビューの要約記録。各ストリームの日付・対象・判定・主な指摘と解消・残件および未検証事項を残す。同じ指摘が 3 回出た場合は、レビューではなく lint・テスト・テンプレートで機械的に防ぐ。

---

## 2026-11-20 / 土台（`7edfadd`）

- **対象**: `crates/` 全体（並行レビュー: 仕様検収・可読性）
- **判定**: 初回 不合格 → 修正 4 ストリーム（`fix/fix-core` / `fix/fix-gui` / `fix/fix-cli` / `fix/fix-docs`）を三段構成で独立検証し、`fix/fix-core` 再検証（`33b3838`）後に全件**合格**
- **主な指摘と解消**:
  - 終了コード 5（安全拒否）の producer 欠如 → `Error::Safety` と網羅 match を追加
  - `PatchPlan::apply` の `before` 未検査、エコー不一致の偽 `offset: 0`（sync プローブ側も `33b3838` でテスト追加）、`Io.path` の `"<unbound>"`、`damaged_blocks` の同名別物、恒等写像でも通る往復テスト、`brom.rs` の定数由来混在を解消
  - ゲート: `./tools/check.sh` 5/5、`cargo test --workspace` 254 件合格

## 2026-11-20 / simulator の `0xD3` 修正（`fix/simulator-d3`）

- **対象**: `simulator.rs`（`a9171fb`、後に `crates/m241-flash/src/simulator.rs` へ移設）
- **判定**: **合格**（`check.sh` 5/5）
- **主な指摘と解消**:
  - `0xD3` は範囲記録のみで消去せず、`0xD5` が書き込み直前に消去するよう修正
  - 応答長（成功 2 バイト `5A 00` / 拒否 1 バイト `A5`）、拒否時の既存範囲保持、および `saturating_sub(FLASH_BASE)` のオフセット飽和バグを修正し、変異テストで確認
- **残件**: `0xD3` の複数レンジ指定は未モデル化

## 2026-11-21 / 読み取り・書き込みの分離（`feat/safety-split` → `fix/safety-split-v2`）

- **対象**: `m241-core`（読み取り専用）と `m241-flash`（書き込み）の分離、`tools/check_safety.py` 追加
- **判定**: 初回 不合格 → Stage 2（`020ac82`）→ Stage 3 独立検証 **合格**（`check.sh` 6/6、282 テスト）
- **主な指摘と解消**:
  - 「コンパイルで書き込み不能を保証」の過大表現を「書き込み・消去ルーチンを持たない（生トランスポートは共有のため公開）」に限定
  - `crates/m241-flash/tests/safety_gate.rs` で `check.sh` 配線とゲート有効性を固定
  - `erase.rs` の ACK 検証を `$PMTK001,182,6,<status>` に厳格化し、`check_safety.py` の検出漏れ（`0xD4`・`\xNN`・`tests/`・`m241-gui` 依存）と行末コメント誤検出を修正

## 2026-11-21 / EXIF 読み書きとジオタグ（`feat/exif2` → `fix/exif2-review`）

- **対象**: `crates/m241-core/src/log/`（`exif.rs`, `geotag.rs`, `fixture.rs`, `mod.rs`）
- **判定**: 初回 不合格 → Stage 2（`a433142`）→ Stage 3 独立検証 **合格**（`check.sh` 6/6、`log/` 74 テスト）
- **主な指摘と解消**:
  - `replace_file` の非 Windows 削除フォールバックおよび再 rename 失敗時の一時ファイル消失バグを修正（`#[cfg(windows)]` 限定・失敗時は一時ファイル保持）
  - JPEG EOI の `0xD9` リテラル衝突を `JPEG_MARKER_SOI + 1` で回避
  - IFD offset 境界、時刻範囲検証、`DateTimeOriginal` の範囲外エラー化、負の高度テストを追加

## 2026-11-21 / GUI 設計レビュー修正（`fix/design-review2` → `fix/design-review2-v2`）

- **対象**: `crates/m241-gui/`
- **判定**: 初回 不合格 → Stage 2（`1f97b24`）→ Stage 3 独立検証 **合格**（`check.sh` 6/6、GUI 93 テスト）
- **主な指摘と解消**:
  - `connect.rs` の数値パネルを右揃え（`common::figure_line`）に統一
  - `StepMarker::is_filled` で塗り／中抜きの形状区別をテスト固定し、`settings.rs` から内部仕様書名露出を除去
  - 空ログフッター非表示、見本バナー整理、左レール固定幅の回帰テストを追加

## 2026-11-21 / CLI 8 コマンド配線（`feat/cli-wire2` → `fix/cli-wire2-v2`）

- **対象**: `crates/m241-cli/`（`device ports/info`, `fw dump/patch`, `log info/download/read`, `nmea repair`）および `docs/reference/02-cli.md`
- **判定**: 初回 不合格 → Stage 2（`9e8bf1c`）→ Stage 3 独立検証 **合格**（`check.sh` 6/6、CLI 108 テスト）
- **主な指摘と解消**:
  - `StreamState::Unknown` を `Error::Safety`（exit 5）へ変更、`$GPZDA` の暦妥当性検証、`checked_add`、ヘッダサイズ警告の分離
  - `log read --utc-offset-days auto` の最新日時判定修正、非フラッシュ `Error::Rejected` の exit 1 分離、`--dry-run` 既パッチ拒否（exit 4）、`--json` テスト追加

## 2026-11-21 / ezTour core 修正（`fix/eztour-core`）

- **対象**: `crates/m241-core/src/log/`（8 ファイル）
- **判定**: Stage 2 修正完了（Must 4 件解消・1 件は事実誤認としてテスト補強のみ）
- **主な指摘と解消**:
  - `KmzPhoto::max_edge_pixels` に SOF ヘッダ検査を追加（超過時は無言通過せず拒否；後に `image` クレートで縮小を実装）
  - 時刻シフト後の自動再マッチング（`plan_with_shift` + `write_datetime_original`）、距離グループ化の union-find 化、`GPSVersionID` 規格外時の `None` 仕様を doc 化

## 2026-11-21 / GUI ログ画面と接続（`feat/gui-eztour-live` → `fix/gui-eztour-live-v2`）

- **対象**: `crates/m241-gui/` および `docs/dev/01-design-decisions.ja.md`
- **判定**: Stage 1 不合格 → Stage 2 → Stage 3 不合格（確認トークン回避など 3 変異生存）→ Stage 2 v2 で解消（`check.sh` 6/6、GUI 132 テスト）
- **主な指摘と解消**:
  - `GeotagConfirmation` を子モジュール `geotag_gate.rs` へ隔離し、UI クリック経路テストで未確認書込を遮断
  - 再生終了後の不要 repaint 停止、片側欠損座標の不変条件、rail の reduced motion 配線、始点・終点マーカーをテスト固定
- **残件・未検証**: スライダーのキーボード/accesskit 操作は未解決。`probe_serial_port`・追従実描画・GUI 経由の実 JPEG 書込は実機未検証

## 2026-11-21 / DA 書き込み経路の独立検証（`fix/da-write-g` → `fix/da-write-g-v2`）

- **対象**: `crates/m241-flash/src/da.rs`, `simulator.rs`
- **判定**: Stage 1/3 不合格 → Stage 2 v2 で解消（`check.sh` 6/6、flash 82 テスト）
- **主な指摘と解消**:
  - `FakeDa` に `read_bytes()` / `read_polls()` を追加し、`0xD3` 拒否後の余分な読み（V11）を検出
  - パケット数カウントの `0xD5` ごとのリセット漏れバグを修正、`EXPECTED_WRITE_START` ピンの変異テスト（V4）を追加
- **未検証**: `0xD3` 1 バイト拒否、`0xD9` 無応答、消去失敗 `0xA5`、ゼロ詰めは命令列のみで実機未検証

## 2026-11-21 / CLI 非破壊 3 コマンド（`fix/cli-non-destructive`）

- **対象**: `crates/m241-cli/`（`device doctor`, `log geotag`, `nmea proxy`）および `docs/reference/02-cli.md`
- **判定**: Stage 1 → Stage 2（`b82f3ce`）→ Stage 3・Windows CI 修正（`d1160a1`）再レビュー **合格**（`check.sh` 6/6、CLI 132 テスト）
- **主な指摘と解消**: 仕様 Must 2・可読性 Must 4・Should 19/20 を解消、Windows パス区切りテストを修正
- **未検証**: 実機 `device doctor` probe、実カメラ JPEG への `log geotag`、Bluetooth SPP の `nmea proxy`

## 2026-11-21 / DA ローダーと `fw probe-da`（`feat/da-loader-probe`）

- **対象**: `DaLoader`（`0xA1`/`0xAF`/`0xA8`）および `m241 fw probe-da`（`0xD6`）
- **判定**: Stage 1 不合格 → Stage 2 / Stage 2 v2 で Must 全件・V-Must-1 解消（`check.sh` 6/6）
- **主な指摘と解消**:
  - `BaseVerdict` を `Settled` / `Inconclusive::{Unread, NoMatch, Ambiguous}` に厳格化し、2 候補とも読めて片方のみ一致した場合だけ確定
  - `0xA8` の count を 0（`DA_JUMP_COUNT`）に固定、DA 先頭 4 バイト `00 00 A0 E1` 検査、`clear_input()` 失敗の伝播、`testing.rs` に end-to-end 偽 DA を追加
  - 偽 DA の `0xD6` 非ゼロ開始アドレスずれバグを修正
- **未検証**: `0xD6` 最終ページ後の advance 送出要否、ページ長可変、V1.14 個体での probe

## 2026-11-22 / PR #19 `fix/flash-d3-resync`

- **対象**: `m241-flash` / `m241-cli` の `0xD3` 応答再同期・自動 `0xD9` 再試行・`fw da-restart`
- **判定**: Stage 1（並行）→ Stage 2 修正完了（`check.sh` 6/6）
- **主な指摘と解消**:
  - `02-cli.md` に `da-restart` 実機未検証を明記、`0xD9` の「再起動した」断定を「送った（未確認）」に修正
  - 再試行前の再入失敗に `--recover` を出さない分岐（`Reentry::BeforeRetry`）、`0xD5` ヘッダ送信失敗時の非再試行、残留 `0x5A` と status `0x00` 検査、8 バイト上限境界テストを追加

## 2026-11-22 / PR #20 `feat-log-kmz`

- **対象**: `crates/m241-cli/`（`log kmz`, `log edit`）および docs
- **判定**: Stage 1（並行）→ Stage 2 修正完了（`check.sh` 6/6）
- **主な指摘と解消**:
  - `log kmz` の写真スキップ理由を `GeotagPlan::skipped` の実理由に変更、`--tz-offset-minutes` 追加、`--utc-offset-days` 適用順テスト追加
  - `log edit` の入力不変性、時刻なしマージ末尾配置、split 派生ファイルの上書き拒否（exit 5）、破損ブロック警告を追加
- **未検証**: Google Earth / `unzip` での実表示確認

## 2026-11-22 / PR #21 `feat-log-edit`

- **対象**: `crates/m241-core/src/log/`（`PhotoTimeShift`, `PhotoShiftScope`, `CameraModel`, `KmzIconOptions`）および docs
- **判定**: Stage 1（並行）→ Stage 2 修正完了（`check.sh` 6/6）
- **主な指摘と解消**:
  - `CameraModel` を厳密 UTF-8・末尾 NUL/空白のみ除去へ変更（非 UTF-8 は `None`）、実カメラ IFD0 配置の未検証を明記し `06-open-questions.md` §6/§7 に記録
  - スコープ別シフトの e2e テスト、overflow のファイル読込前拒否、JPEG セグメント走査の `exif` 統合、`export_kmz_inner` 分割を実施

## 2026-11-22 / PR #23 `feat-geotag-cli`

- **対象**: `crates/m241-cli/`（`log geotag` の `--shift-*` / `--scope` / `--photo` / `--camera-*`）および docs
- **判定**: Stage 1（並行）→ Stage 2 修正完了（`check.sh` 6/6）
- **主な指摘と解消**:
  - mtime 由来写真のシフト非永続化を警告・doc 化、`--scope selected` の 0 件一致を `Error::State`（exit 1）化、シフト要約を `requested` 表記へ変更
  - スコープ外フラグの警告テスト、tz 併用 e2e テスト、`--scope` 綴り定数化、`flags_for_other_scopes()` への一本化を実施

## 2026-11-22 / PR #24 W2a DA 全文 SHA-256（`feat-da-digest`）

- **対象**: `m241-flash` / `m241-cli` の DA ダイジェスト検証と `tools/check_safety.py`
- **判定**: Stage 1（並行）→ Stage 2 修正完了（`check.sh` 6/6）
- **主な指摘と解消**:
  - 生 `&str` を `DaDigest` トークン（通常ビルドは `DaDigest::vendor()` のみ）に置換し、`DaDigest::for_test_da` の `#[cfg]` 制限を `check_safety.py` 検査 8 に追加
  - CI で常に走る pin 値リテラル回帰テストを追加し、ベンダー DA 実ファイル依存テストの条件付き skip を `11-safety-review.md` に明記

## 2026-11-22 / PR #26 1.0.0 リリース整備（`feat-release-prep`）

- **対象**: `README.md`, `CHANGELOG.md`, `docs/user/01-install.md`, `05-verification.md`, `12-progress-summary.md`, `.github/workflows/release.yml`
- **判定**: Stage 1（並行）→ Stage 2 修正完了（`check.sh` 6/6、当時 810 テスト）
- **主な指摘と解消**:
  - `nmea proxy` 実機未検証・`v1.0.0` タグ未作成・個体データ 14/4/1 バイトの関係・Linux glibc/libudev 前提・未署名バイナリ注意を明記
  - `release.yml` のタグ制約（`v[0-9]+.[0-9]+.[0-9]+`）、版一致検査、`cargo test --locked`、`LICENSE` 同梱を追加
- **見送り**: 署名 / build provenance、composite action 化

## 2026-11-22 / PR #27 GUI デザイン整備（`feat-gui-design`）

- **対象**: `crates/m241-gui/` および `DESIGN.md` §10
- **判定**: Stage 1（仕様・可読性・デザインの 3 レビュー）→ Stage 2・Stage 3 軽微対応完了（`check.sh` 6/6、GUI 152 テスト）
- **主な指摘と解消**:
  - `WaitingForPowerCycle` の進捗 100% 保持、設定「地図キャッシュの場所」行追加、`common::focus_ring`（2pt ink フォーカス輪）、`common::tool_button` 統一、`Shadow::NONE`、rail の quadratic ease-out、「動きの低減」表記統一、コピーの括弧化を実施
  - `DESIGN.md` §10 に書体同梱方針（SIL OFL 1.1・非同梱）と 10 色軌跡の設計判断を記録

## 2026-11-22 / PR #28 ezTour P2 互換（`feat-p2-compat`）

- **対象**: `m241-core` / `m241-cli`（NMEA インポート、プロジェクト保存/読込、CSV 出力）および docs
- **判定**: Stage 1（並行）→ Stage 2 修正完了（`check.sh` 6/6）
- **主な指摘と解消**:
  - プロジェクト保存の相対パスをプロジェクト親基準に統一し、`"input": "dump" | "nmea"` と必須フィールド検証を追加
  - 非空 NMEA 全却下を `Error::State` 化、`--format json` へ `nmea` カウント付与、`$GPGSA` 等の `ignored`/`dropped` 境界修正、自己上書き拒否（exit 5）を追加
- **未検証**: 実 ezTour 出力との突き合わせ

## 2026-11-23 / PR #31 GUI 配線（`feat-gui-wiring`）

- **対象**: `crates/m241-gui/`（写真シフト・byCamera・KMZ 書き出し・ログ編集）および `DESIGN.md`
- **判定**: Stage 1（並行）→ Stage 2 修正完了（`check.sh` 6/6、GUI 169 テスト）
- **主な指摘と解消**:
  - 再計画失敗時の一覧保持＋`plan_stale` 拒否、byCamera 基準カメラ表示・変更時再計画、シフト arm 中の KMZ 拒否、範囲内/外枚数の報告、`見本` キャプション統一、編集 UI の 1 始まり統一を実施
- **未検証**: Google Earth / `unzip`、実写真・実カメラでの byCamera 一致

## 2026-11-23 / PR #34 ログの設定ジャーナル（`fix-log-journal`）

- **対象**: `m241-core` / `m241-cli` の設定ジャーナル解析と stale mask 対応
- **判定**: Stage 1（並行）→ Stage 2 修正完了（`check.sh` 6/6）
- **主な指摘と解消**:
  - 実測の journal type（2/3/4/6/7、計 248 件）を正確に反映、`record::decode_with_length` で mask 20 B / 実長 16 B ブロックを救済
  - 再同期の連続チェックサム要件を名前付き定数化（宣言 2 / 非宣言 3 / 再同期 4、ランダム 50 seeds で 0 点）、`is_journal_only` 定義を厳格化、`02-cli.md` に `log info` 節を追加

## 2026-11-23 / PR #36 ezTour パリティ core/CLI（`feat-parity-core`）

- **対象**: `m241-core` / `m241-cli`（KML `LookAt`、キャッシュ、`--gpx`、`--simplify`、`--csv-report`、`--coord-format`、NMEA GGA/GSA）
- **判定**: Stage 1（並行）→ Stage 2 修正完了（`check.sh` 6/6、936 テスト）
- **主な指摘と解消**:
  - KML `<LookAt>` を `<Style>` 前に移動（最小 XSD + `xmllint` で検証済）、`logcache.rs` 分離と `ParseReport` 警告リプレイ（v2 キャッシュ）
  - `--gpx` で `auto` 日数補正を無効化（歴史的 GPX の誤シフト防止）し 0 点 GPX をエラー化、GGA 衛星数を空欄へ変更、`TrackPoint::position()` で有限座標判定を統一
- **未検証**: 外部ビューア描画、GPX/NMEA 出力の外部検証

## 2026-11-23 / PR #37 GUI ログ/NMEA 操作（`feat-gui-logops`）

- **対象**: `crates/m241-gui/`（吸い出し・消去パネル・プロジェクト・NMEA 修復/中継）および docs
- **判定**: Stage 1（並行）→ Stage 2 → PR #36/#38 追従 → Stage 3 再検証（`d14a129`）**合格**（`check.sh` 6/6、GUI 226 テスト、CLI と JSON 出力の raw 一致を確認）
- **主な指摘と解消**:
  - プロジェクト相対パス規則・JSON の `nmea`/`offset_days`/`input`/`position` を CLI と統一
  - `LogOps` をパネル別 struct に分割し、KMZ icon を既定 OFF へ戻し、`--port auto` のケーブル形状フィルタと中継の最大行数制限を追加
- **残件・未検証**: geotag の `--tz-offset-minutes` 相当は follow-up。GUI 経由の実機吸い出し・中継・消去は未検証

## 2026-11-23 / PR #38 GUI 書き込み（`feat-gui-write`）

- **対象**: `crates/m241-flash` / `crates/m241-gui` / `tools/check_safety.py`（Firmware 書き込み・2 回読み吸い出し・Connect 診断/probe-da）
- **判定**: Stage 1（並行）→ Stage 2 → Stage 3 検証（`43af066`）**合格**（17 変異全検出・誤検出 0、`review-gate: success`；その後 main 追従マージ）
- **主な指摘と解消**:
  - `check_safety.py` を 9 検査へ拡張（GUI 破壊的オペコードの 2 系統走査、`Confirmation::granted` / `Job::Flash` のファイル allowlist）
  - GUI 吸い出しを 2 回読み＋ハッシュ一致検証へ実装、preflight のメモリ保持で TOCTOU 解消、未検証表示の統一、`probe_da` / `probe_bootrom` / `settled_address` テスト追加
- **未検証**: GUI 経由の実機書き込み・2 回読み（シミュレータのみ）

## 2026-11-23 / PR #39 OSM タイル地図（`feat-map-tiles`）

- **対象**: `crates/m241-gui/`（`tiles.rs`, `log.rs`, `settings.rs`）および `DESIGN.md` / `PRODUCT.md` / `DISCLAIMER.md` / `README.md`
- **判定**: Stage 1（並行）→ Stage 2 → PR #37 競合解消マージ完了（`check.sh` 6/6、GUI 282 テスト）
- **主な指摘と解消**:
  - 取得ゲートを `native_pixels_per_point.is_some()` へ変更（Wayland/X11 実ウィンドウで取得、headless/sample は fetch 0・キャッシュ 0）
  - タイル描画枚数 > 0 のみ帰属クレジット描画、`image::Limits` によるデコード前上限、`BodyExceedsLimit` の `TooLarge` 変換、失敗タイルの 60 秒後再試行、`pub(crate) mod tiles` 化
- **残件・未検証**: HTTP キャッシュヘッダ（`max-age`/ETag）・失敗理由の詳細表示・設定永続化は見送り。Wayland 実画面の見た目と実 `.bin` でのライブ表示は未検証

## 2026-11-23 / PR #41 ezTour 形式（`feat-eztour-formats`）

- **対象**: `crates/m241-core/src/log/`（`import.rs`, `html.rs`, `report.rs`）, `crates/m241-cli/src/cli.rs`, `docs/reference/02-cli.md`
- **判定**: Stage 1（並行）→ Stage 2 → Stage 3（`c0b4c5d`）**合格** → PR #40 追従＋軽微 2 件修正（`check.sh` 6/6）
- **主な指摘と解消**:
  - KML 読込を O(n) 1 パス化（10,000 点: 7.198 s → 0.011 s）、ZIP 展開を `Read::take(64 MiB + 1)` 制限（160 MiB 展開 RSS: 197.1 MiB → 69.1 MiB）、暗号化 ZIP と ZIP64 の拒否、ZIP 異常系テスト追加
  - HTML 禁止トークン 16 種検査・RTF 全体検証・CRC 未検証の明記・MHT 未実装節を追加
- **未検証**: ブラウザ/Word での表示確認、実 ezTour 出力との比較

## 2026-11-23 / PR #42 Log の地図主面化（`feat-map-layout`）

- **対象**: `crates/m241-gui/`（`log.rs`, `drawer.rs`, `tiles.rs`, `theme.rs`, `screenshot.rs`）および `DESIGN.md`
- **判定**: Stage 1（並行）→ Stage 2 → Stage 3（`01f682a`）**合格** → PR #41 追従＋軽微 2 件修正（`check.sh` 6/6、GUI 292 テスト）
- **主な指摘と解消**:
  - 帰属板を `Plates::credit` へ移しズーム板左端で折り返し（最小間隔 12.6pt、ドロワー判定を content 幅 1000pt 基準へ変更）、浮遊ドロワーの重なり回避、長文エラーの折り返し対応
  - `--screenshot` にドロワー 5 タブ × 2 サイズの 10 枚を追加、10 色巡回・選択点マーカー・板 alpha 0.92 の描画テストを追加
- **未検証**: 実機、実ネットワークのタイル取得、実ウィンドウでの見た目

## 2026-11-23 / PR #43 Tauri v2 GUI（`feat-tauri-p0`）

- **対象**: `crates/m241-app/`（Rust バックエンド + `ui/`）、`tools/check_safety.py`、`tools/check.sh`、docs
- **判定**: Stage 1（仕様・安全・可読性・デザイン）→ Stage 2 修正完了（`check.sh` 6/6、`m241-app` 28 テスト、`safety_gate` 35 テスト、クリーンチェックアウト確認済み）
- **主な指摘と解消**:
  - `check_safety.py` に `m241-app` の 3 条項（`m241-flash` 禁止、破壊的コマンド禁止、`Port::open` 1 箇所限定）を追加し、4 種の変異で失敗を実証
  - `check.sh` に `ui/dist` 自動ビルド段を追加、同期コマンドを `async fn` + `spawn_blocking` 化、symlink/hard link 経由の自己上書き拒否を追加
  - 写真マッチングにカメラ時差・ログ日数ずれ・許容差・距離グループ化を追加、地図占有率 53.3% → 61.25%（1280×800）、OSM ラスタ減彩（平均彩度 0.105 → 0.051）、CSP 設定と a11y フォーカス管理を実施
- **未検証**: 実機、実シリアルポート、Tauri 実ウィンドウ（CSP・IME・OS ダイアログ）、Windows / macOS、実写真フォルダでのカメラ時差一致
