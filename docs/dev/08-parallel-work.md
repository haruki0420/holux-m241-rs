# 次の作業（サブエージェント割り当て用）

このファイルは作業指示書ではなく、**並列に走らせるときの分割と依存関係**の記録。
各担当は自分の範囲のファイルだけを書き、共通ファイル（`Cargo.toml`、
`docs/dev/*`）は統合時に触る。

## 完了済み（`main` 統合済み・三段構成検証済み）

- **土台・読み書き分離**: `m241-core`（読み取り専用）/ `m241-flash`（書き込み専用: `LogEraser` + DA スタブ + `FakeDa`）/ `m241-cli` / `m241-gui`、CI 3 OS、`./tools/check.sh` 6 ゲート（`fmt` / `clippy` / `test` / `docs` / `design` / `safety`）
- **A（DA 逆解析）**: [`docs/reference/01-da-protocol.md`](../reference/01-da-protocol.md) 完了（725 行、全 6 問い回答・命令アドレスつき）
- **B（ezTour 仕様化）**: [`docs/dev/07-eztour-spec.ja.md`](07-eztour-spec.ja.md) 完了（615 行、クリーンルーム仕様と P0/P1/P2 優先順位）
- **C（先行実装差分）**: [`docs/dev/04-prior-art.md`](04-prior-art.md) 完了
- **D（コア読み取り系）**: PMTK / BootROM / プロファイル / パッチ計画 / ログ解析・GPX・KML・CSV・NMEA・JSON 書き出し完了
- **E（GUI 4 画面 + 設計レビュー修正）**: 接続 / ログ / ファーム / 設定画面の実装と設計レビュー修正完了（サンプル表示・ヘッドレス描画テスト 93 件）
- **F（写真ジオタグ基盤）**: `crates/m241-core/src/log/exif.rs`・`geotag.rs` 完了（Stage 3 検証済み）
- **CLI 8 コマンド配線**: `device ports` / `device info`、`fw dump` / `fw patch`、`log info` / `log download` / `log read`、`nmea repair` 完了（108 テスト）

## 次に並列で進めるストリーム

このプロジェクトは **① ファームウェア修復（安全最優先）** と **② 公式 ezTour v3.0 のクリーンルーム再実装（[`07-eztour-spec.ja.md`](07-eztour-spec.ja.md) §8）** の両輪で進める。

| # | 範囲 | 主な書き込み先 | 依存・前提 |
|---|---|---|---|
| G | **DA 書き込み実装**（`0xD3`/`0xD5`/`0xD9`・読み戻し検証） | `crates/m241-flash/src/da.rs`、`simulator.rs` | `AGENTS.md` §2.2・§2.3・`11-safety-review.md` の安全要件を一切弱めない設計方針の確定が先決 |
| H | **CLI 残りコマンド配線**（`log erase` / `log geotag` / `device doctor` / `nmea proxy`、G 完了後に `fw flash`） | `crates/m241-cli/src/`、`docs/reference/02-cli.md` | `fw flash` のみ G に依存 |
| I | **ezTour P0: KMZ 書き出し**（KML + 写真同梱 + 長辺縮小 zip） | `crates/m241-core/src/log/` | なし（`07-eztour-spec.ja.md` §6.3・§8 P0） |
| J | **ezTour P0: GUI 実データ接続・再生・写真同期**（サンプル専用から実ログ/実ポート/実ファイル接続、全点 fit、追従、再生、POI 表示） | `crates/m241-gui/src/` | なし（`DESIGN.md`・`07-eztour-spec.ja.md` §5・§8 P0） |
| K | **ezTour P1: 写真ジオタグ拡充**（EXIF `GPSVersionID`・`_original` バックアップ・距離グループ化・時刻シフト再マッチ） | `crates/m241-core/src/log/exif.rs`、`geotag.rs` | なし（`07-eztour-spec.ja.md` §6・§8 P1） |
| L | **ezTour P1/P2: トラック編集・読込互換**（分割/結合/点削除/Undo・GPX/NMEA 読込・座標形式切替） | `crates/m241-core/src/log/`、`crates/m241-gui/src/` | J の後に GUI 統合（`07-eztour-spec.ja.md` §8 P1/P2） |

## 共通の決まり

- `./tools/check.sh` が 6/6 でない状態でコミットしない
- `clippy::unwrap_used` / `expect_used` / `panic` / `unsafe` / `indexing_slicing` はテスト以外で禁止
- コメントは「何を」ではなく「なぜ」。特に安全ゲートの理由は消さない
- 実機で見ていないものは 未検証 と書く。`docs/dev/05-verification.md` が唯一の昇格先
- 個人データ（実座標、ローカルパス、実機の COM 名、ベンダーバイナリ）をコミットしない
- 終了したワークツリーと `target/` は速やかに片付ける（`AGENTS.md` §9.6）

## レビュー（三段構成）

[`09-review-protocol.md`](09-review-protocol.md) と `AGENTS.md` §8 の三段構成（発見 → 別エージェントが修正 → 三人目が説明を読まずに独立検証）を例外なく守る。合格条件は次の 3 つ。

1. 指摘がすべて解消し、変異テスト等で退行検出が裏付けられている
2. 新たな問題・安全後退・テスト削除が入っていない
3. `./tools/check.sh` が 6/6 で通る
