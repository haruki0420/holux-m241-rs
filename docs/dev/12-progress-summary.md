# 進捗サマリ — holux-m241-rs

**このファイルは現在の到達点、残っている課題、読むべき一次資料をまとめた記録。**
最終更新: 2026-11-22 JST

---

## 0. まず読む一次資料（この順で）

| 順 | ファイル | なぜ |
|---|---|---|
| 1 | [`AGENTS.md`](../../AGENTS.md) | 作業規約。**§2 安全規則**、**§6 個人情報**、**§8 三段構成**、**§9 失敗の記録** は必読 |
| 2 | [`PRODUCT.md`](../../PRODUCT.md) / [`DESIGN.md`](../../DESIGN.md) | 誰のために何を作るか、画面設計とデザイン不変条件 |
| 3 | [`docs/dev/07-eztour-spec.ja.md`](07-eztour-spec.ja.md) | **公式 ezTour v3.0 の振る舞い仕様**（クリーンルーム再実装の P0/P1/P2 基準） |
| 4 | [`docs/dev/05-verification.md`](05-verification.md) | **何が実機で検証済みで、何が未検証か**（唯一の昇格先） |
| 5 | [`docs/reference/01-da-protocol.md`](../reference/01-da-protocol.md) / [`docs/dev/11-safety-review.md`](11-safety-review.md) | 書き込み経路・読み書き分離を触るなら必読 |
| 6 | [`docs/dev/04-prior-art.md`](04-prior-art.md) / [`docs/dev/06-open-questions.md`](06-open-questions.md) | 先行実装との差分と未解決の問い |

---

## 1. 何を作っているか（2 本の柱）

**Holux M-241**（MediaTek MT3318 の GPS ロガー、2006 年製）のための、単一ワークスペースのクロスプラットフォーム Rust ツール（CLI + GUI）。

1. **WNRO（GPS ウィークナンバーロールオーバー）の安全な修復**
   - 2006 年製ファームの GPS エポック定数 `315964800`（`0x402F4`）を `935280000` へ書き換え（実際に変わるのは 3 バイト）、19.6 年（7168 日）の日付ずれを直す。
   - 書き込み処理は先行 Python 実装からの移植ではなく、`DA_v3.8.4.bin` の逆アセンブリ（[`01-da-protocol.md`](../reference/01-da-protocol.md)）から再構築する。
2. **死んだ公式ソフト（HOLUX ezTour for Logger v3.0）のクリーンルーム再実装**
   - 公式 ezTour は Google Maps JS API v2/v3 終了・Holux / iTravel-Tech 認証サーバ停止・外部 `exiftool.exe` 依存により動かない（[`07-eztour-spec.ja.md`](07-eztour-spec.ja.md) §7）。
   - 認証なし・SNS 連携なし・自前地図・自前 EXIF で、ログ取得・地図表示・再生・GPX/KML/KMZ/CSV 変換・写真ジオタグ・トラック編集を現代に蘇らせる（同 §8）。

---

## 2. 現在の到達点（1.0.0 リリース時点）

### 数字とゲート

| 項目 | 値 |
|---|---|
| バージョン | **1.0.0**（`Cargo.toml` の `[workspace.package]` 1 か所で 4 クレートを統一。初回公開リリース） |
| クレート構成 | 4 クレート（`m241-core` 読取専用 / `m241-flash` 書込専用 / `m241-cli` / `m241-gui`） |
| テスト | **810 件全緑**（2026-11-22 に `cargo test --workspace --offline -- --list` で実測。内訳は m241-core 297 / m241-flash 149 / m241-cli 232 / m241-gui 132） |
| ゲート | **6/6 PASS**（`./tools/check.sh`: fmt / clippy / test / docs / design / safety。2026-11-22 に `M241_VENDOR_IMAGE` を設定して実行）。`M241_VENDOR_IMAGE` はベンダーの V1.13 ファームウェアイメージのパスで、設定するとその隣の `DA_v3.8.4.bin` を使う実 DA 受理テストが走る（未設定・CI では skip） |
| CI | Linux / Windows / macOS（`ci.yml`） |
| 配布 | `.github/workflows/release.yml`: `v1.0.0` のような安定版タグで 3 OS の `cargo build --release` を行い、`m241` / `m241-gui` / `LICENSE` / `SHA256SUMS.txt` を GitHub Release に添付。**まだ一度も実行していない**（`v*` タグ・Release とも未作成） |
| **実機検証** | 読み取りと `log erase`（[05-verification.md](05-verification.md) §1a）に加え、2026-11-22 の書き込みセッション（同 §1.5）で `fw dump` / `fw probe-da` / `fw patch` / `fw flash --recover` / 通常の書き込みが実機で通り、**2 回の書き込みが全長読み戻し一致**。`fw flash --allow-vendor` は書き込みが完走したが読み戻しは不一致（設計課題）。残りは**測位ありでの日付確認**と GUI の実機検証。詳細は §3.3 |

### 完成・`main` 統合済み（三段構成で検証済み、[`10-review-log.md`](10-review-log.md) 記録済み）

| 部分 | 状態 |
|---|---|
| **DA 逆解析** | 完了（`docs/reference/01-da-protocol.md` 725 行、命令アドレスつき） |
| **ezTour 仕様化** | 完了（`docs/dev/07-eztour-spec.ja.md` 615 行、クリーンルーム仕様） |
| **読み書き分離 + safety ゲート** | 完了（`m241-core` / `m241-flash` 分離、`tools/check_safety.py` + 配線・検出力テスト 19 件） |
| **コア読み取り・変換・EXIF/ジオタグ基盤** | 完了（PMTK / BootROM / ログ解析 / GPX・KML・CSV・NMEA・JSON 出力 / パッチ計画 / 自前 EXIF・geotag） |
| **CLI 17 コマンド** | 完了（`device ports` / `info` / `doctor`、`fw dump` / `patch` / `flash` / `probe-da` / `da-restart`、`log info` / `download` / `read` / `edit` / `erase` / `geotag` / `kmz`、`nmea repair` / `proxy`。`fw flash` は 2026-11-22 に実機で書き込み・読み戻しまで通った（[`05-verification.md`](05-verification.md) §1.5）。`log erase` は PR #12 で配線済み、実機記録は同 §1a。`log edit` / `log kmz` は PR #20 で追加。スタブは残っていない） |
| **GUI 4 画面（実データ接続・再生・写真同期）** | 完了（接続 / ログ / ファーム / 設定。実ポート probe・実ログ読込・再生・写真同期・POI・始点終点・設定反映。132 テスト。実機未検証） |

- **2026-11-21 のマージ**: review-gate (#2)、CLI 非破壊 3 コマンド (#3)、GUI 実データ接続 (#4)、
  ezTour core + KMZ 縮小 (#5)、DA 書き込み経路（当時は開始アドレス未確定のため拒否）(#6)。

---

## 3. 残っている課題

### 3.1 ファーム書き込み・残りの CLI コマンド

| 項目 | 状態 | ブロッカー / 注意点 |
|---|---|---|
| **DA 書き込み実装**（`crates/m241-flash/src/da.rs`） | 実装済み。開始アドレスは確定済み（PR #13） | 実機では書込み・消去・再起動・`0xD9` 後の再入・全長読み戻しまで通過（[05-verification.md](05-verification.md) §1.5）。ベンダーイメージの読み戻しだけが設計課題。詳細は下の補足 |
| **CLI 配線** | 完了（スタブなし） | `fw flash` は `verify_device` → DA RAM ロード → `0xD3`/`0xD5` → `0xD9` → NMEA probe + `$PMTK180*3B` で BootROM 再入 → 読み戻し照合まで配線済み。`0xD3` 失敗時は `0xD9` で再起動し `0xD5` 前だけ 1 回再試行。`log erase` は PR #12 で実装済み（実機記録は §1a）。`device doctor` / `log geotag` / `nmea proxy` はマージ済みだが実機未検証。詳細は下の補足 |

補足:

- DA 側の開始アドレスは 2026-11-21 の `fw probe-da` で `0` に確定した（[05-verification.md](05-verification.md) §1.2）。`UNRESOLVED_STARTS` は空で、`check_layout` は確定値 `0` を通す。
- `--da` は長さ（14,896 バイト）・先頭 4 バイト・全文 SHA-256（`DA_IMAGE_SHA256`、`AGENTS.md` §2.5 のベンダー値）の 3 つを照合する（follow-up W2a を実装済み）。期待ダイジェストは `DaDigest` トークンで、通常ビルドの構築子は `DaDigest::vendor()` のみ（テスト用 `for_test_da` は `simulator` feature 配下）。
- 通常の受理規則は自機バックアップとの差分のみ（同一または `PATCH_OFFSETS` のみ）。`--allow-vendor` は固定 V1.13 SHA-256 の公式イメージだけを受理する別ゲートで、個体データ 14 バイトを上書きする。実機では 2026-11-22 に書き込みが完走し V1.13 で起動したが、起動したファームが個体データ 4 バイトを復元するため読み戻しは `0x5C` で不一致（同 §1.5）。
- `write_range` は private。読み戻し・消去失敗 `0xA5`・ゼロ詰め・`0xD9` 無応答はテスト済み（dry-run の個体データ上書き警告、再入失敗を「読み戻し未実施」として不一致と区別する報告もテスト済み）。`fw da-restart` は DA のまま残った端末に単独の `0xD9` を送る。

### 3.2 ezTour 代替機能の再実装（[`07-eztour-spec.ja.md`](07-eztour-spec.ja.md) §8）

| 優先度 | 機能 | 現状と不足 |
|---|---|---|
| **P0** | **KMZ 書き出し** | マージ済み（PR #5 / #20 / #21）。写真同梱（`photo/`）＋ zip 化＋色/幅＋ `<Point>`／`<TimeStamp>`＋長辺縮小（`kmzMaxPhotoEdgeSize`）＋ `icon/` サムネイルを実装。CLI 経路は `log kmz --max-photo-edge` 等で接続済み。**未検証**: 実写真での縮小結果、EXIF Orientation 付き写真の向き、Google Earth / `unzip` での開封。詳細は下の補足 |
| **P0** | **GUI の実データ接続・地図体験** | マージ済み（PR #4）。実ポート probe・実ログ読込・全点 fit・追従（zoom ≥ 12）・再生・写真同期・POI・始点終点を実装。**未検証**: 実ポート probe、追従の実描画、GUI 経由の実 JPEG 書き込み。スライダーのキーボード / accesskit 操作は未対応（見送り理由を記録済み） |
| **P1** | **写真ジオタグ拡充** | `exif.rs` / `geotag.rs` に実装済み: `GPSVersionID`・`_original` バックアップ・距離グループ化（推移的／順序非依存）・時刻シフト後の再マッチング（`plan_with_shift` で再計画、`apply` が `DateTimeOriginal` を書き換え、次回はシフト無しで一致）。PR #21 で日/時/分/秒の個別シフト（`PhotoTimeShift`、各値は符号付き・合計は checked 演算）と適用範囲（`PhotoShiftScope`: 既定 `AllPhotos` / `SelectedPhotos` / `ByCamera`）を `plan_with_time_shift` として追加し、`plan_with_shift` はその `AllPhotos` 版への委譲にした（既存動作の回帰テストあり）。`byCamera` は Exif IFD0 の `Make`/`Model` を `CameraModel::from_jpeg` で読む。CLI は `log geotag`（dry-run / `--tz-offset-minutes` / シフト / バックアップ）、GUI は写真同期と書き込み確認まで実装済み。**未確定**: 「同じカメラ」の判定規則（対象タグ・大文字小文字・表記揺れの正規化。現状は末尾の NUL/空白だけを除いた完全一致で、UTF-8 として読めない名前は不明扱い）、`selPhotos` の一致規則（現状はパスの完全一致）、日 = 24 時間（カレンダー日ではない）の解釈。詳細は [`06-open-questions.md`](06-open-questions.md) §6。新しい scoped API は CLI/GUI 未配線（別担当）。未実装: 非 JPEG（動画）の「紐づけのみ」。実カメラ JPEG への書き込みと、実カメラの IFD0 配置（合成 JPEG でしか読めていない）は**未検証** |
| **P1** | **トラック編集** | 分割（Split）・結合（Merge）・点削除・Undo/Redo は `edit.rs` に実装済み（PR #5）。CLI 経路は PR #20 で追加（`log edit --split-at` / `--merge` / `--delete`）。GUI の操作体系（範囲選択）は未着手 |
| **P2** | **互換・移行機能** | GPX 1.0/1.1 読込（`<wpt>` は読まない）・NMEA 読込→トラック化・座標形式切替（度分秒 / 10 進度 / UTM）は実装済み（PR #5）。`feat-p2-compat` で (1) NMEA 読込が `NmeaImport`（`imported` / `dropped` / `ignored`）を返し、`m241 log read --nmea` が「読み込んだ行数／落ちた行数」を stderr に表示、(2) CSV の列・順を ezTour の 12 列（INDEX / TRACK NUMBER / UTC DATE / UTC TIME / LOCAL DATE / LOCAL TIME / LATITUDE / N/S / LONGITUDE / E/W / ALTITUDE / SPEED）に一致、(3) `m241 log project save\|load` で log・写真フォルダ・時刻補正・出力設定を 1 つの JSON に保存し、`log read` / `log kmz` 相当の出力を行う、まで実装。**未確定**: CSV の `LOCAL DATE` / `LOCAL TIME` は、ezTour が表示用タイムゾーンのどれを基準にしていたか仕様に無いため**空で出力**（UTC や PC のゾーンを推測して書かない）。`INDEX` の基点（0 始まり）・`TRACK NUMBER` の値（1）・日時/緯度経度/高度/速度の書式・N/S・E/W の綴りも仕様に無く、こちらで固定した値（テストで固定）。プロジェクト JSON は `.itm` の互換ではなく本プロジェクトの最小形式（[`docs/reference/02-cli.md`](../reference/02-cli.md) に明記） |

補足（KMZ）:

- 長辺縮小は `KmzPhoto::max_edge_pixels` が `Some(limit)` のときだけ JPEG を復号し、アスペクト比を保って Lanczos3 で縮小、JPEG 品質 85 で再符号化する。上限以内と `None` の写真は元のバイト列のまま。縮小した写真は EXIF を保持しない（`image` の JPEG エンコーダは APP1 を書かない）。上限超過で復号できないバイト列は `Error::State`（写真名と理由つき）。
- `icon/` は `export_kmz_with_icons` が別上限（`KmzIconOptions::max_edge_pixels`）で同じ縮小経路から生成し、`export_kmz` は `icon/` を書かない（回帰テストあり）。CLI フラグは `--max-photo-edge` / `--utc-offset-days` / `--tz-offset-minutes` / `--max-gap`。
- **未確定**: サムネイルの寸法（仕様に記載なし）、`icon/` の命名規則、KML からの参照方法、ezTour が実際に `icon/` へ置いていたか。**残る未実装**: `kmzMaxPhotoEdgeSize`・`icon/` 寸法を GUI から渡す経路。依存は `image` の `jpeg` feature のみ。

### 3.3 実機検証の到達点

読み取りと書き込みの主要経路は実機で通った。結果は [`05-verification.md`](05-verification.md) §1.5 に記録：

- `fw dump`（2 回一致）、`fw probe-da`（DA 側ベース `0`）、`fw patch`（3 バイト）
- `fw flash --recover` と通常の書き込み（自機バックアップとの差分のみを許す経路、パッチ済み V1.14）: 1589 パケット完走、406,620 バイト読み戻し一致
- `fw flash --allow-vendor`（ベンダー V1.13）: 書込みは完了したが、起動したファームが個体データ 4 バイトを復元するため読み戻しが `0x5C` で不一致（設計課題）
- V1.12 の書き込み拒否を実測

残る実機検証: 測位ありでの日付確認（屋内では既定値のみ）、GUI（実ポート probe・追従描画・GUI 経由の JPEG 書き込み）、他のユニット形状（V1.12・4 MiB 品・GR-245 系）、Bluetooth SPP。

---

## 4. 絶対に守る規約

- **三段構成（`AGENTS.md` §8）**: 発見 → 別エージェントが修正 → 三人目が説明を読まずに独立検証。進行役も例外なし。
- **安全ゲートを弱めない（`AGENTS.md` §2）**: NMEA 事前三値プローブ、`0xD5` の範囲定数と事前検証、バックアップ差分チェック、明示確認。
- **シミュレータは正しさの証明ではない（[`05-verification.md`](05-verification.md) §3）**: 回帰テストの道具。実機未検証のものは必ず「未検証」と書く。
- **個人データ・ベンダーバイナリ禁止（`AGENTS.md` §6）**: 実座標・ローカルパス・実ポート列挙・`DA_v3.8.4.bin` 等のコミット厳禁。
- **ワークツリーと `target/` の整理（`AGENTS.md` §9.6）**: 終わったワークツリーは残さず片付ける。

---

## 5. コマンド早見表

```bash
./tools/check.sh              # 6 ゲート（fmt / clippy / test / docs / design / safety）
cargo test --workspace        # 全テスト（現在 810 件）
cargo build --release
cargo run -q -p m241-gui -- --screenshot
```
