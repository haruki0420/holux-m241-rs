# 次の作業（サブエージェント割り当て用）

このファイルは作業指示書ではなく、**並列に走らせるときの分割と依存関係**の記録。
各担当は自分の範囲のファイルだけを書き、共通ファイル（`Cargo.toml`、
`docs/dev/*`）は統合時に触る。

## 土台（完了）

- `m241-core`: `frame` / `port` / `brom` / `error` / `firmware::image` / `log::record`
- `m241-cli`: サブコマンド体系（`device|fw|log|nmea`）と終了コード対応
- `m241-gui`: レール（階段状タブ）、トークン、ヘッドレス描画テスト
- CI 3 OS、`./tools/check.sh` 5 ゲート

## 並列に走らせる 6 本

| # | 範囲 | 書き込み先 | 依存 |
|---|---|---|---|
| A | DA 逆解析 → `docs/reference/01-da-protocol.md` | `docs/reference/01-da-protocol.md`、解析メモは `_scratch/` | なし |
| B | ezTour 仕様書 → `docs/dev/07-eztour-spec.ja.md`（未作成） | そのファイルのみ | なし | <!-- path-check: ignore -->
| C | 上流差分表 → `docs/dev/04-prior-art.md` 追記 | そのファイルのみ | なし |
| D | コア読み取り系（PMTK、`fw dump`、`log download`、`log read`） | `crates/m241-core/src/protocol/pmtk.rs`、`log/`、`firmware/profiles.rs` | なし |
| E | GUI 3画面（接続・ログ・ファーム） | `crates/m241-gui/src/screens/*` | D（型が要る。スタブで開始可） |
| F | 写真ジオタグ（EXIF 自前実装） | `crates/m241-core/src/log/` 配下に exif / geotag を追加 | D | <!-- path-check: ignore -->

**A が終わるまで flash の実装は始めない。** `m241-flash/src/da.rs` は A の成果物を
引用できない限り書かない、が現行のルール（`CONTRIBUTING.md`）。

**E は D の型に依存する。** D が先に型（`DeviceInfo`、`TrackPoint` など）を
置いてから E を本格化する。スタブで画面だけ先に作るのは可。

## 共通の決まり

- `./tools/check.sh` が 5/5 でない状態でコミットしない
- `clippy::unwrap_used` / `expect_used` / `panic` / `unsafe` はテスト以外で禁止
- コメントは「何を」ではなく「なぜ」。特に安全ゲートの理由は消さない
- 実機で見ていないものは 未検証 と書く。`docs/dev/05-verification.md` が唯一の昇格先
- 個人データ（実座標、ローカルパス、実機の COM 名）をコミットしない

## レビュー

実装者とは別のエージェントが、**仕様書とテストだけを見て**検収する。
実装者の説明は読まない。合格条件は次の 3 つ。

1. テストが通る
2. `clippy -D warnings` が通る
3. 仕様書の項目をすべて満たす（項目ごとに根拠を示す）

## 画像生成について

comp（構図の見本）は ChatGPT / Grok とも未ログインで作れなかった。
代わりに egui のシェルを先に作り、実画面をスクリーンショットして
`design-review` に渡す方針（`DESIGN.md` §8）。
