# レビュー記録

[`09-review-protocol.md`](09-review-protocol.md) の手順で行ったレビューの結果。
対象・日付・指摘・対応・検証を 1 レビュー 1 節で残す。

同じ指摘が 3 回出たら、レビューではなく仕組み（lint、テスト、テンプレート）で塞ぐ。

---

## 2026-11-20 / 土台（`7edfadd` 時点のコード一式）

対象: `crates/` 全体。2 種類のレビューを**互いの結果を見せずに**並行で走らせた。

### 仕様検収 — 不合格

判定の根拠は「仕様が要求する安全ゲートが、機能が無いことだけに依存している」こと。

- `docs/reference/01-da-protocol.md` は仕様書自体が空（逆解析待ち）。実装の `da.rs` が
  常に失敗を返す点は整合しているが、検収できる仕様が存在しない。
- `docs/reference/02-cli.md` の終了コード表のうち **5（安全拒否）に対応する分岐が
  コードに無い**。文書化されたコードに producer が無いのは未文書より悪い。
- 13 コマンド中 12 が未実装スタブ。`--json` / `--dry-run` / `--yes` / `--backup` /
  `--require-backup` / `--port` のフラグが clap 定義に無い。
- 「検出するテスト」として認められなかったもの: 恒等写像でも通る往復テスト、
  `shapes > 0` だけの描画テスト、`checksum_xor16`（本番呼出元ゼロ）。
- 仕様外の実装として `types.rs` / `brom.rs` / `frame.rs` / `port.rs` / GUI クレート
  全体が挙がった（01 は DA、02 は CLI のみを規定しているため）。

### 可読性レビュー — Must 8 件

1. `types.rs:188` — `PatchPlan::apply` が `PatchEdit::before` を読まずに書く。
   doc の「レビュー済みの計画」と `a_plan_built_for_another_image_is_refused` の
   テスト名が嘘になる。
2. `brom.rs:194` — エコー不一致に `offset: 0` を与えており、位置を持たない失敗に
   偽の座標を表示する。
3. `error.rs:175` — `Io.path` が `"<unbound>"` で埋まり `i/o on <unbound>: …` が出る。
4. `types.rs:92` と `:128` — `damaged_blocks` が `Vec<u32>` と `usize` で同名別物。
5. `theme.rs:7` — 存在しない `tests/design_invariants.rs` を根拠に書いている。
6. `m241-gui/Cargo.toml:16` — `m241-core` が未使用依存。
7. `brom.rs:7` — 「実機で確立した定数」と宣言した直後に参照実装由来・別チップ由来の
   値が混在し、全部が実機検証済みに読める。
8. `frame.rs:87` — 往復テストが恒等写像でも通る。

Should 10 件、Could 2 件。全文は各修正担当への指示に転記した。

### 対応

修正は**別のエージェントに割り振り**、その修正を**さらに別のエージェントが独立に
レビューする**三段構成にした（実装 → 修正 → 検証）。理由: 指摘した本人が直すと、
直したつもりの箇所を同じ思い込みで読み直す。

| 修正ストリーム | 範囲 | 検証ストリーム | 検証の状態 |
|---|---|---|---|
| `fix/fix-core` | `crates/m241-core/src/` 全体 | 未割当 | 未検証 |
| `fix/fix-gui` | `crates/m241-gui/` | 未割当 | 未検証 |
| `fix/fix-cli` | `crates/m241-cli/`（core の `Error::Safety` 追加後） | 未割当 | 未検証 |
| `fix/fix-docs` | `docs/`、`AGENTS.md`、`tools/` | 未割当 | 未検証 |

検証ストリームは、修正ストリームに**参加していない**エージェントが担当する
（[`09-review-protocol.md`](09-review-protocol.md) の三段構成）。検証担当が読むのは
指摘リストと修正の差分とテストだけで、修正者の説明は読まない。合格条件は
「指摘がすべて解消 / 新たな問題を入れていない / `./tools/check.sh` が 5/5」の 3 つ。
状態は `未検証` → `合格` / `不合格` で更新する。

---

## 2026-11-20 / simulator の `0xD3` 修正（`fix/simulator-d3`）

対象: `crates/m241-core/src/protocol/simulator.rs`。ブランチ `fix/simulator-d3`、
HEAD `a9171fb`、分岐点 `e58c5d3`（main は検証中に `5d6ad74` まで進んだ。
`git diff main..HEAD` は main 側の `AGENTS.md` 追加分の逆差分を混ぜるので、
ブランチの実体は `git diff e58c5d3..HEAD` の 1 ファイルである）。

### 指摘の解消

| # | 指摘 | 解消 | 根拠 |
|---|---|---|---|
| 1 | `0xD3` は記録だけで消去しない | 解消 | `simulator.rs:662-682`（`erase_range` の呼び出し無し、`da_range` 代入のみ） |
| 2 | 消去は `0xD5` が書き込み直前に行う | 解消 | `simulator.rs:702-703`（`erase_range` の唯一の呼び出し） |
| 3 | 応答長が成功 2 バイト / 拒否 1 バイト | 解消 | 成功 `simulator.rs:682`、拒否 `simulator.rs:674` |
| 4 | 拒否された `0xD3` は以前のレンジを残す | 解消 | 拒否枝が `simulator.rs:666-676` で return し、代入 `:677` に到達しない |
| 5 | doc コメント「(and erases it)」が誤り | 解消 | `simulator.rs:71-75` が「records the range and nothing else」に修正済み（旧は main の `simulator.rs:51`） |

### 変異テスト（実際にコードを壊して実行し、毎回 `git checkout --` で戻した）

| 変異 | 落ちたテスト | 期待どおり |
|---|---|---|
| `0xD3` に消去を戻す | `recording_a_memory_range_leaves_the_flash_untouched`（1 件のみ） | はい |
| `0xD5` の消去を削除 | `the_write_command_erases_...`、`a_refused_memory_range_keeps_...`（2 件） | はい |
| `0xD5` の消去を最初のパケット後に移す | `the_write_command_erases_...`、`a_refused_memory_range_keeps_...`、既存の `a_write_packet_is_acknowledged_and_stored`（計 3 件） | はい（順序違反で落ちる） |
| 拒否応答を `[NAK, STATUS_OK]` | `a_refused_memory_range_is_answered_with_one_nak_byte_and_nothing_else`、`a_refused_memory_range_keeps_the_range_recorded_before_it`（2 件） | はい |
| 拒否時に `da_range` を上書き | `a_refused_memory_range_keeps_the_range_recorded_before_it` | はい |
| 消去をバイト単位に | `the_write_command_erases_...`、`a_refused_memory_range_keeps_...`（2 件） | はい |
| 書き込みオフセットを `saturating_sub(FLASH_BASE)` に戻す | `a_refused_memory_range_keeps_the_range_recorded_before_it` のみ（`simulator.rs:1231` で payload が `0xFF` のまま） | はい（ただし専用テストは無く、付随的に検出） |
| 参考: 成功応答を 3 バイトに | 4 件（拒否 2 件・パケット 2 件） | はい |

### 副次修正（旧コードの確認）

`git show main:crates/m241-core/src/protocol/simulator.rs` の `simulator.rs:689` は
`let offset = self.da_cursor.saturating_sub(FLASH_BASE) as usize;` で、`da_cursor` は
`0xD3` のリニアな `start`。`FLASH_BASE = 0x0020_0000` なのでフラッシュ全域で 0 に飽和し、
実際に常に 0 番地へ書いていた。主張は正しく、誇張ではない。なお同じ誤りが
`erase_range`（旧 `:704-705`）にもあり、こちらは書き換えで一緒に直っている。

### 新たに入った問題

なし（コード側）。補足:

- アサーションの削除・弱体化は無い（差分に `-` の assert 行が 1 行も無い）。
  既存テストの書き換えは 4 つのヘルパー（`download_agent` / `memory_range` /
  `read_reply` / `write_packet`）への置換のみで、期待値は同じか厳しくなっている
  （`read_reply` で応答長まで比較するようになった）。テストは 19 → 23 件で削除なし。
- `FLASH_SECTOR_SIZE = 0x4000` は `simulator.rs:59-69` に「記述子が未解読なので
  モデル自身の選択」と明記されている。ただし仕様 §6 は記述子の先頭ワードを列挙しており
  `0x00004000` が 2 回現れる（`docs/reference/01-da-protocol.md:601`）。「cite できない」
  はやや強いが、値の裏付けが §6 にあること自体は問題ない。
- `accepted_da_range` の `start > end` 拒否（`simulator.rs:851`）は、仕様 §4.2 の
  3 条件をそのまま写したものではないが、§4.2 は判定を `FUN_00100A34(start, end-start+1)`
  と書いており（`docs/reference/01-da-protocol.md:226-227`）、`start > end` なら長さが
  巨大になって写像範囲チェックで落ちるので、拒否は整合する。
- `0xD5` は実機の「到達したセクタだけを書く直前に消す」ではなく、レンジ全体を
  コマンド受理時に先に消す（`simulator.rs:689-696` に意図として明記）。転送が
  中断した場合のフラッシュ状態は実機と変わるが、テストの対象は正常完了経路なので
  実害は無い。ただし「when a sector is erased is not visible on the wire」という
  コメント（`simulator.rs:695-696`）は、途中で止めた場合には当てはまらない。
- `0xD3` のカウントバイト（複数レンジ）は旧実装から未モデル化のまま
  （`needed = 10` 固定、`simulator.rs:647`）。今回の指摘の範囲外。

### 仕様書との整合

`docs/reference/01-da-protocol.md` §4.6 は「The second `0x5A` is sent only after that
erase succeeded (`0x00102560`)」と、消去が 2 つ目の `5A` の**前**だと明記している。
§5.6 の表でも「erase the sector holding the first range's start」→「erase succeeded →
`0x5A`」の順。修正後の `simulator.rs:702-705` は消去してから `[ACK, ACK]` を積むので
一致する（実機は 1 つ目の `5A` を消去前に返すが、フェイクは両方まとめて積むだけで
ワイヤ上のバイト列は同じ）。

### ゲート

`./tools/check.sh` = **5/5**（fmt / clippy / test / docs / design、25 秒）。

### 判定

**合格**。指摘 5 点はすべてコード上で解消され、追加 4 テストは対応する変異で
実際に落ち、既存テストの弱体化も無い。
