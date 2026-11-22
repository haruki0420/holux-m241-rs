# 進捗サマリ — holux-m241-rs

**このファイルは現在の到達点、残っている課題、読むべき一次資料をまとめた記録。**
最終更新: 2026-11-23 JST

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
| テスト | **1066 件全緑**（2026-11-23 に `cargo test --workspace --offline -- --list` で実測。内訳は m241-core 338 / m241-flash 166 / m241-cli 279 / m241-gui 282 の 1065 件に、doc-test 1 件を加えたもの） |
| ゲート | **6/6 PASS**（`./tools/check.sh`: fmt / clippy / test / docs / design / safety。2026-11-23 に `M241_VENDOR_IMAGE` を設定して実行）。`M241_VENDOR_IMAGE` はベンダーの V1.13 ファームウェアイメージのパスで、設定するとその隣の `DA_v3.8.4.bin` を使う実 DA 受理テストが走る（未設定・CI では skip） |
| CI | Linux / Windows / macOS（`ci.yml`） |
| 配布 | `.github/workflows/release.yml`: `v1.0.0` のような安定版タグで 3 OS の `cargo build --release` を行い、`m241` / `m241-gui` / `LICENSE` / `SHA256SUMS.txt` を GitHub Release に添付。**まだ一度も実行していない**（`v*` タグ・Release とも未作成） |
| **実機検証** | 読み取りと `log erase`（[05-verification.md](05-verification.md) §1a）に加え、2026-11-22 の書き込みセッション（同 §1.5）で `fw dump` / `fw probe-da` / `fw patch` / `fw flash --recover` / 通常の書き込みが実機で通り、**2 回の書き込みが全長読み戻し一致**。`fw flash --allow-vendor` は書き込みが完走したが読み戻しは不一致（設計課題）。GUI の接続画面と「接続して確認」は 2026-11-22 に実機で成功し、`接続済み（読み取り専用）`・`B-core_1.20` / `0043` / `01017-00F` / SDK 1.0・ログ使用量 3,292 B を表示した（同 §1.5）。GUI の Log 画面は 2026-11-23 に**開発機の実ウィンドウ（X11）**で 2026-11-21 の実ダンプ（10,942 点）を開き、OSM タイル・軌跡・帰属表示が同時に出ることを確認した（**M-241 実機とは無関係**。Wayland は実ウィンドウでのタイル取得のみ。同 §1.5）。残りは**測位ありでの日付確認**、**GUI からの実機書き込み**（経路は実装済み・未検証）、GUI の**追従描画・GUI 経由の JPEG 書き込み**、Log 画面の KMZ 書き出し・写真の時刻シフト・トラック編集 UI（いずれも実機・実写真で未検証）。詳細は §3.3 / §3.4 |

### 完成・`main` 統合済み（三段構成で検証済み、[`10-review-log.md`](10-review-log.md) 記録済み）

| 部分 | 状態 |
|---|---|
| **DA 逆解析** | 完了（`docs/reference/01-da-protocol.md` 725 行、命令アドレスつき） |
| **ezTour 仕様化** | 完了（`docs/dev/07-eztour-spec.ja.md` 615 行、クリーンルーム仕様） |
| **読み書き分離 + safety ゲート** | 完了（`m241-core` / `m241-flash` 分離、`tools/check_safety.py` + 配線・検出力テスト 19 件） |
| **コア読み取り・変換・EXIF/ジオタグ基盤** | 完了（PMTK / BootROM / ログ解析 / GPX・KML・CSV・NMEA・JSON 出力 / パッチ計画 / 自前 EXIF・geotag）。ログ解析は PR #34 で設定ジャーナル対応（`journal.rs`）: 2026-11-21 の 225,028 B ダンプで 10,942 点 / レコードチェックサム不一致 0 / journal 248、書き込み後の 2 ダンプは 0 点 / journal-only（設定ジャーナルのみ）。旧パーサは 11,088 点中 4,481 点を不一致のまま点として出していた |
| **CLI 17 コマンド** | 完了（`device ports` / `info` / `doctor`、`fw dump` / `patch` / `flash` / `probe-da` / `da-restart`、`log info` / `download` / `read` / `edit` / `erase` / `geotag` / `kmz`、`nmea repair` / `proxy`。`fw flash` は 2026-11-22 に実機で書き込み・読み戻しまで通った（[`05-verification.md`](05-verification.md) §1.5）。`log erase` は PR #12 で配線済み、実機記録は同 §1a。`log edit` / `log kmz` は PR #20 で追加。スタブは残っていない） |
| **GUI 4 画面（接続・ログ・ファーム・設定）** | 完了（PR #4 / #31 / #37 / #38 / #39。282 テスト）。**ファーム画面**は GUI から吸い出し（2 回読み＋SHA-256 一致）→ PC 上でパッチ→`WRITE` のタイプ入力→`m241_flash::flow` の共有セッションで書き込み（進捗）→全長読み戻し→復旧まで行い、`--allow-vendor` 相当・`PROBE` / `RESTART` も GUI から使える。自動リトライはしない。安全ゲートは「GUI は flash 非依存」から「GUI は `flow::flash` 経由・`WRITE` / `PROBE` 必須・破壊的オペコード混入禁止」に変わった（`tools/check_safety.py`）。**実機からの GUI 書き込みは未検証**。**ログ画面**は実ポート probe・実ログ読込・再生・写真同期・POI・始点終点に加え、ログ操作コンソール（吸い出し / 消去はバックアップ＋同意の二重ゲートで、実行は別承認のため拒否で止まり CLI のみ / プロジェクト保存・読込 / NMEA 修復 / NMEA 中継（TCP）/ 読み書き（GPX 読込含む））、写真の距離グループ化・`_original` 既定 ON、集計（移動時間 / 最高速度 / 高度範囲）、表示用の座標形式、KMZ icon（既定 OFF）・カメラ時差 / 許容、KMZ 書き出し・写真シフト・トラック編集を持つ。**地図**は OSM ラスタタイル（Web Mercator、ディスクキャッシュ 30 日 / 200 MB、同時 4、可視＋1 リング）を既定 ON で描き、帰属はタイルを描いたときだけ、headless は非取得、オフラインは模式キャンバス。設定画面で ON/OFF・キャッシュの場所 / サイズ / 消去。2026-11-23 に開発機の X11 実ウィンドウで実ダンプ 10,942 点の軌跡・タイル・帰属の同時表示を確認（M-241 非接続）。GUI からの実機書き込み・吸い出し / 消去 / 中継・追従描画・実写真は未検証（詳細は §3.4） |

- **2026-11-21 のマージ**: review-gate (#2)、CLI 非破壊 3 コマンド (#3)、GUI 実データ接続 (#4)、
  ezTour core + KMZ 縮小 (#5)、DA 書き込み経路（当時は開始アドレス未確定のため拒否）(#6)。

---

## 3. 残っている課題

### 3.1 ファーム書き込み・残りの CLI コマンド

| 項目 | 状態 | ブロッカー / 注意点 |
|---|---|---|
| **DA 書き込み実装**（`crates/m241-flash/src/da.rs`） | 実装済み。開始アドレスは確定済み（PR #13） | 実機では書込み・消去・再起動・`0xD9` 後の再入・全長読み戻しまで通過（[05-verification.md](05-verification.md) §1.5）。ベンダーイメージの読み戻しだけが設計課題。詳細は下の補足 |
| **CLI 配線** | 完了（スタブなし） | `fw flash` は `verify_device` → DA RAM ロード → `0xD3`/`0xD5` → `0xD9` → NMEA probe + `$PMTK180*3B` で BootROM 再入 → 読み戻し照合まで配線済み。`0xD3` 失敗時は `0xD9` で再起動し `0xD5` 前だけ 1 回再試行。`log erase` は PR #12 で実装済み（実機記録は §1a）。`device doctor` / `log geotag` はマージ済みだが実機未検証。`nmea proxy` は 2026-11-22/23 に実機の live ストリームの中継（stdout と TCP `--bind`）で成功し、長時間連続・複数クライアント・非ループバック bind・fix ありストリームは未検証。詳細は下の補足 |

補足:

- DA 側の開始アドレスは 2026-11-21 の `fw probe-da` で `0` に確定した（[05-verification.md](05-verification.md) §1.2）。`UNRESOLVED_STARTS` は空で、`check_layout` は確定値 `0` を通す。
- `--da` は長さ（14,896 バイト）・先頭 4 バイト・全文 SHA-256（`DA_IMAGE_SHA256`、`AGENTS.md` §2.5 のベンダー値）の 3 つを照合する（follow-up W2a を実装済み）。期待ダイジェストは `DaDigest` トークンで、通常ビルドの構築子は `DaDigest::vendor()` のみ（テスト用 `for_test_da` は `simulator` feature 配下）。
- 通常の受理規則は自機バックアップとの差分のみ（同一または `PATCH_OFFSETS` のみ）。`--allow-vendor` は固定 V1.13 SHA-256 の公式イメージだけを受理する別ゲートで、個体データ 14 バイトを上書きする。実機では 2026-11-22 に書き込みが完走し V1.13 で起動したが、起動したファームが個体データ 4 バイトを復元するため読み戻しは `0x5C` で不一致（同 §1.5）。この扱いの選択肢は [`06-open-questions.md`](06-open-questions.md) §8（決定はユーザー）。
- `write_range` は private。読み戻し・消去失敗 `0xA5`・ゼロ詰め・`0xD9` 無応答はテスト済み（dry-run の個体データ上書き警告、再入失敗を「読み戻し未実施」として不一致と区別する報告もテスト済み）。`fw da-restart` は DA のまま残った端末に単独の `0xD9` を送る。

### 3.2 ezTour 代替機能の再実装（[`07-eztour-spec.ja.md`](07-eztour-spec.ja.md) §8）

| 優先度 | 機能 | 現状と不足 |
|---|---|---|
| **P0** | **KMZ 書き出し** | マージ済み（PR #5 / #20 / #21）。写真同梱（`photo/`）＋ zip 化＋色/幅＋ `<Point>`／`<TimeStamp>`＋長辺縮小（`kmzMaxPhotoEdgeSize`）＋ `icon/` サムネイルを実装。CLI 経路は `log kmz --max-photo-edge` 等で接続済み。**未検証**: 実写真での縮小結果、EXIF Orientation 付き写真の向き、Google Earth / `unzip` での開封。詳細は下の補足 |
| **P0** | **GUI の実データ接続・地図体験・書き込み・ログ操作** | マージ済み（PR #4 / #31 / #37 / #38 / #39）。実ポート probe・実ログ読込・全点 fit・追従（zoom ≥ 12）・再生・写真同期・POI・始点終点、OSM 地図、GUI からのファーム書き込みとログ操作まで入った。できることと未検証の範囲は §3.4 にまとめた。2026-11-23 に開発機の X11 実ウィンドウで実ダンプの軌跡・タイル・帰属の同時表示を確認（M-241 非接続）。**未検証**: GUI からの実機書き込み、GUI からの吸い出し / 消去 / 中継、追従の実描画、GUI 経由の実 JPEG 書き込み、KMZ 書き出し・写真シフト・トラック編集 UI（実機・実写真・Google Earth で未確認）、Wayland の実画面での地図の見た目。スライダーのキーボード / accesskit 操作は未対応（見送り理由を記録済み） |
| **P1** | **写真ジオタグ拡充** | `exif.rs` / `geotag.rs` に実装済み: `GPSVersionID`・`_original` バックアップ・距離グループ化（推移的／順序非依存）・時刻シフト後の再マッチング（`plan_with_shift` で再計画、`apply` が `DateTimeOriginal` を書き換え、次回はシフト無しで一致）。PR #21 で日/時/分/秒の個別シフト（`PhotoTimeShift`、各値は符号付き・合計は checked 演算）と適用範囲（`PhotoShiftScope`: 既定 `AllPhotos` / `SelectedPhotos` / `ByCamera`）を `plan_with_time_shift` として追加し、`plan_with_shift` はその `AllPhotos` 版への委譲にした（既存動作の回帰テストあり）。`byCamera` は Exif IFD0 の `Make`/`Model` を `CameraModel::from_jpeg` で読む。CLI は `log geotag`（dry-run / `--tz-offset-minutes` / シフト / バックアップ）、GUI は写真同期と書き込み確認まで実装済み。**未確定**: 「同じカメラ」の判定規則（対象タグ・大文字小文字・表記揺れの正規化。現状は末尾の NUL/空白だけを除いた完全一致で、UTF-8 として読めない名前は不明扱い）、`selPhotos` の一致規則（現状はパスの完全一致）、日 = 24 時間（カレンダー日ではない）の解釈。詳細は [`06-open-questions.md`](06-open-questions.md) §6。scoped API は CLI（`log geotag --scope all|selected|by-camera` と `--shift-*`）と、PR #31 で GUI（写真シフトの適用範囲）に配線済み。未実装: 非 JPEG（動画）の「紐づけのみ」。実カメラ JPEG への書き込みと、実カメラの IFD0 配置（合成 JPEG でしか読めていない）は**未検証** |
| **P1** | **トラック編集** | 分割（Split）・結合（Merge）・点削除・Undo/Redo は `edit.rs` に実装済み（PR #5）。CLI 経路は PR #20 で追加（`log edit --split-at` / `--merge` / `--delete`）。GUI は PR #31 で Log 画面に配線（範囲は数値入力。ドラッグでの範囲選択は無い）。実機未検証 |
| **P2** | **互換・移行機能** | GPX 1.0/1.1 読込（`<wpt>` は読まない）・NMEA 読込→トラック化・座標形式切替（度分秒 / 10 進度 / UTM）は実装済み（PR #5）。`feat-p2-compat` で (1) NMEA 読込が `NmeaImport`（`imported` / `dropped` / `ignored`）を返し、`m241 log read --nmea` が「読み込んだ行数／落ちた行数」を stderr に表示、(2) CSV の列・順を ezTour の 12 列（INDEX / TRACK NUMBER / UTC DATE / UTC TIME / LOCAL DATE / LOCAL TIME / LATITUDE / N/S / LONGITUDE / E/W / ALTITUDE / SPEED）に一致、(3) `m241 log project save\|load` で log・写真フォルダ・時刻補正・出力設定を 1 つの JSON に保存し、`log read` / `log kmz` 相当の出力を行う、まで実装。**未確定**: CSV の `LOCAL DATE` / `LOCAL TIME` は、ezTour が表示用タイムゾーンのどれを基準にしていたか仕様に無いため**空で出力**（UTC や PC のゾーンを推測して書かない）。`INDEX` の基点（0 始まり）・`TRACK NUMBER` の値（1）・日時/緯度経度/高度/速度の書式・N/S・E/W の綴りも仕様に無く、こちらで固定した値（テストで固定）。プロジェクト JSON は `.itm` の互換ではなく本プロジェクトの最小形式（[`docs/reference/02-cli.md`](../reference/02-cli.md) に明記） |

補足（KMZ）:

- 長辺縮小は `KmzPhoto::max_edge_pixels` が `Some(limit)` のときだけ JPEG を復号し、アスペクト比を保って Lanczos3 で縮小、JPEG 品質 85 で再符号化する。上限以内と `None` の写真は元のバイト列のまま。縮小した写真は EXIF を保持しない（`image` の JPEG エンコーダは APP1 を書かない）。上限超過で復号できないバイト列は `Error::State`（写真名と理由つき）。
- `icon/` は `export_kmz_with_icons` が別上限（`KmzIconOptions::max_edge_pixels`）で同じ縮小経路から生成し、`export_kmz` は `icon/` を書かない（回帰テストあり）。CLI フラグは `--max-photo-edge` / `--utc-offset-days` / `--tz-offset-minutes` / `--max-gap`。
- **未確定**: サムネイルの寸法（仕様に記載なし）、`icon/` の命名規則、KML からの参照方法、ezTour が実際に `icon/` へ置いていたか。**残る未実装**: `kmzMaxPhotoEdgeSize`・`icon/` 寸法を GUI から渡す経路。依存は `image` の `jpeg` feature のみ。

### 3.3 実機検証の到達点

読み取りと書き込みの主要経路は実機で通った。結果は [`05-verification.md`](05-verification.md) §1.5 に記録：

- `fw dump`（2 回一致）、`fw probe-da`（DA 側ベース `0`）、`fw patch`（3 バイト）
- `fw flash --recover` と通常の書き込み（自機バックアップとの差分のみを許す経路、パッチ済み V1.14）: 1589 パケット完走、406,620 バイト読み戻し一致
- `fw flash --allow-vendor`（ベンダー V1.13）: 書込みは完了したが、起動したファームが個体データ 4 バイトを復元するため読み戻しが `0x5C` で不一致（設計課題）。選択肢 A/B/C と安全ゲートへの影響、推奨は [`06-open-questions.md`](06-open-questions.md) §8。決定はユーザー
- V1.12 の書き込み拒否を実測

GUI の到達点（詳細は §3.4）:

- 接続画面と「接続して確認」（2026-11-22、実機・読み取り専用）: `接続済み（読み取り専用）`、`B-core_1.20` / `0043` / `01017-00F` / SDK 1.0、ログ使用量 3,292 B
- Log 画面 + OSM 地図（2026-11-23、**開発機**の X11 実ウィンドウ。M-241 は非接続）: 2026-11-21 の実ダンプ 10,942 点を開き、タイル・軌跡・帰属表示 `© OpenStreetMap contributors` が同時に描画された。Wayland の実ウィンドウでもタイル取得（キャッシュ 8 枚）を確認。Wayland の実画面の見た目はスクリーンコピーが応答せず未検証
- GUI からの実機書き込み、GUI からの吸い出し・消去・NMEA 中継: **未検証**

残る実機検証: 測位ありでの日付確認（屋内では既定値のみ）、**GUI からの実機書き込み**、GUI からの吸い出し・消去・NMEA 中継、GUI の追従描画・GUI 経由の JPEG 書き込み・KMZ 書き出し・写真シフト・トラック編集 UI（実機・実写真・Google Earth で未確認）、Wayland の実画面での地図の見た目、`nmea proxy` の長時間連続・複数クライアント・非ループバック bind・fix ありストリーム、他のユニット形状（V1.12・4 MiB 品・GR-245 系）、Bluetooth SPP。

### 3.4 GUI でできること（PR #37 / #38 / #39）

**書き込み（ファーム画面）** — CLI と同じ `m241_flash::flow` の共有セッションを通る。

- 吸い出しは 2 回読み＋SHA-256 一致（1 回目のあと電池を抜き差し）。パッチは PC 上のファイルに対して行い、書き込みは `WRITE` のタイプ入力（`--yes` 相当の省略経路はない）。
- 書き込みは進捗を出し、完了後に全長を読み戻して照合する。復旧（`--recover` 相当）とベンダーイメージ（`--allow-vendor` 相当）も `WRITE` を要求する。`PROBE`（DA 確認）と `RESTART`（`0xD9`）は接続画面・書き込み失敗後から実行できる。
- CLI の `0xD5` 前の 1 回自動リトライは GUI にはない（失敗時はユーザーが再起動・BootROM 再入・復旧を選ぶ）。
- 安全ゲートは「GUI は flash 非依存」から「GUI は `flow::flash` 経由・`WRITE` / `PROBE` 必須・破壊的オペコード混入禁止」に変わった（`tools/check_safety.py`）。**GUI からの実機書き込みは未検証**。

**ログ操作（ログ画面の「ログ操作」コンソール）**

- 吸い出し（PMTK182。中断しても core が録画を再開する）。
- 消去はバックアップ＋同意の二重ゲートまでで、**実行は別承認が必要なため GUI は拒否で止まる**（実施は CLI の `log erase`）。
- プロジェクト保存・読込（CLI と同じ JSON）、NMEA 修復、NMEA 中継（TCP。既定は loopback）、読み書き（`log read` 相当。GPX 読込を含む）。
- 写真: 距離によるグループ化、`_original` バックアップ既定 ON。集計（移動時間・最高速度・高度範囲）を表示し、座標形式は設定画面の表示切替だけ（JSON は 10 進度のまま）。KMZ: `icon/` は既定 OFF、カメラ時差と許容。
- これらは**実機・実写真で未検証**（CLI の同じコア関数は実機で動いているが、GUI 経由では動かしていない）。

**地図（ログ画面）**

- OSM ラスタタイルを `ureq` で取得する（Web Mercator、可視範囲＋1 リング、同時 4、ディスクキャッシュは 30 日 / 200 MB、失敗タイルは 60 秒後に再試行）。
- 帰属 `© OpenStreetMap contributors` はタイルを実際に描いたフレームにだけ出す。`--screenshot` と headless 描画はタイルを取得しない（見本フレームは「見本：地図タイルは表示しません」）。
- オフライン時は模式キャンバスに戻り「地図タイルを取得できません（オフライン）」と表示するが、軌跡・選択・再生・書き出しは動く。
- 設定画面で地図タイルの ON/OFF（**既定 ON・インターネットが必要**）と、キャッシュの場所・サイズ・消去。
- 実ウィンドウでの取得は 2026-11-23 に X11（Xwayland）と Wayland で確認した。X11 では 2026-11-21 の実ダンプ 10,942 点の軌跡・タイル・帰属が同時に出ることを確認。**いずれも開発機の表示確認で、M-241 の実機検証ではない**。Wayland の実画面の見た目は未検証。

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
cargo test --workspace        # 全テスト（現在 1066 件）
cargo build --release
cargo run -q -p m241-gui -- --screenshot
```
