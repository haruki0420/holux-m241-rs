# 進捗サマリ — holux-m241-rs

**このファイルは現在の到達点、残っている課題、読むべき一次資料をまとめた記録。**
最終更新: 2026-11-21 JST

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

## 2. 現在の到達点（`main`）

### 数字とゲート

| 項目 | 値 |
|---|---|
| クレート構成 | 4 クレート（`m241-core` 読取専用 / `m241-flash` 書込専用 / `m241-cli` / `m241-gui`） |
| テスト | **483 件全緑**（`cargo test --workspace`） |
| ゲート | **6/6 PASS**（`./tools/check.sh`: fmt / clippy / test / docs / design / safety） |
| CI | Linux / Windows / macOS |
| **実機検証** | **ゼロ（未検証）** |

### 完成・`main` 統合済み（三段構成で検証済み、[`10-review-log.md`](10-review-log.md) 記録済み）

| 部分 | 状態 |
|---|---|
| **DA 逆解析** | 完了（`docs/reference/01-da-protocol.md` 725 行、命令アドレスつき） |
| **ezTour 仕様化** | 完了（`docs/dev/07-eztour-spec.ja.md` 615 行、クリーンルーム仕様） |
| **読み書き分離 + safety ゲート** | 完了（`m241-core` / `m241-flash` 分離、`tools/check_safety.py` + 配線・検出力テスト 12 件） |
| **コア読み取り・変換・EXIF/ジオタグ基盤** | 完了（PMTK / BootROM / ログ解析 / GPX・KML・CSV・NMEA・JSON 出力 / パッチ計画 / 自前 EXIF・geotag） |
| **CLI 8 コマンド** | 完了（`device ports` / `info`、`fw dump` / `patch`、`log info` / `download` / `read`、`nmea repair`、108 テスト） |
| **GUI 4 画面（レイアウト・設計レビュー修正）** | 接続 / ログ / ファーム / 設定 4 画面の UI と設計ゲート（93 テスト、サンプル表示まで） |

---

## 3. 残っている課題

### 3.1 ファーム書き込み・残りの CLI コマンド

| 項目 | 状態 | ブロッカー / 注意点 |
|---|---|---|
| **DA 書き込み実装**（`crates/m241-flash/src/da.rs`） | 未実装（拒否スタブのみ） | `AGENTS.md` §2.2・§2.3 および [`11-safety-review.md`](11-safety-review.md) の安全要件（範囲定数・リセットベクタ保護・バックアップ差分チェック・読み戻し検証・citation）を一切弱めずに設計確定すること |
| **CLI 残り 5 コマンド** | 未実装スタブ | `log erase`・`log geotag`・`device doctor`・`nmea proxy` は着手中／着手可。`fw flash` は `da.rs` 完了後 |

### 3.2 ezTour 代替機能の再実装（[`07-eztour-spec.ja.md`](07-eztour-spec.ja.md) §8）

| 優先度 | 機能 | 現状と不足 |
|---|---|---|
| **P0** | **KMZ 書き出し** | 写真同梱（`photo/`）＋ zip 化（Store）＋ 色/幅＋ `<Point>`／`<TimeStamp>` は実装済み。**長辺縮小（`kmzMaxPhotoEdgeSize`）は未実装**: JPEG を復号・再符号化するコーデックがこのクレートに無く、依存追加は方針として保留。現状は上限値を受け取り、超える写真を**縮小せずに `Error::State` で拒否**する（黙って原寸を入れることはない）。実装には `image` 等の JPEG codec 依存（Stage 1 修正の報告で提案）か、外部プロセスによる縮小のどちらかが要る |
| **P0** | **GUI の実データ接続・地図体験** | サンプル描画のみ。実ポート接続・実ログ読込・全点 fit・現在位置追従（zoom ≥ 12）・再生（Play）・写真同期・POI 表示が未実装 |
| **P1** | **写真ジオタグ拡充** | `exif.rs` / `geotag.rs` に実装済み: `GPSVersionID`・`_original` バックアップ・距離グループ化（推移的／順序非依存）・時刻シフト後の再マッチング（`plan_with_shift` で再計画、`apply` が `DateTimeOriginal` を書き換え、次回はシフト無しで一致）。未実装: 写真タイムゾーン変換（§6.1-2、呼び出し側の責務のまま）、日/時/分/秒の個別シフトと `byCamera`/`selPhotos`/`allPhotos` の適用範囲、非 JPEG（動画）の「紐づけのみ」、CLI/GUI 連携 |
| **P1** | **トラック編集** | 分割（Split）・結合（Merge）・点削除・Undo/Redo は `edit.rs` に実装済み。GUI の操作体系（範囲選択）は未着手 |
| **P2** | **互換・移行機能** | GPX 1.0/1.1 読込（`<wpt>` は読まない）・NMEA 読込→トラック化・座標形式切替（度分秒 / 10 進度 / UTM）は実装済み。CSV 列の ezTour 互換確認・プロジェクト保存が未着手。NMEA 読込は落ちた行数を返さない（doc に明記） |

### 3.3 実機検証（まだ一度も実施していない）

最初は**読み取りのみ**（危険ゼロ）から開始し、結果は [`05-verification.md`](05-verification.md) に記録する：

```bash
m241 device ports
m241 device info
m241 fw dump -o backup/stock.bin
m241 fw patch backup/stock.bin --dry-run
```

書き込み検証はその次。バックアップから復旧できることも必ず確認する。

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
cargo test --workspace        # 全テスト（現在 483 件）
cargo build --release
cargo run -q -p m241-gui -- --screenshot
```
