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
| `fix/fix-core` | `crates/m241-core/src/` 全体 | verify-merged → sync-probe-test | 不合格 → **合格**（`33b3838`） |
| `fix/fix-gui` | `crates/m241-gui/` | verify-merged | 合格 |
| `fix/fix-cli` | `crates/m241-cli/`（core の `Error::Safety` 追加後） | verify-merged | 合格 |
| `fix/fix-docs` | `docs/`、`AGENTS.md`、`tools/` | verify-merged | 合格 |

不合格の理由: `fix/fix-core` — Must 2 の引用行（`crates/m241-core/src/protocol/brom.rs` の sync プローブ側 `Framing`）に検出テストが無く、`offset: None` を `Some(0)` に戻す変異が全テストを通過していた（エコー側のテストだけでは引用行を守れない）。

**解消** (`33b3838`, 別エージェント): `a_sync_probe_mismatch_does_not_claim_to_be_at_offset_zero` と `every_probe_byte_that_disagrees_reports_no_position` を追加。変異を入れると 2 件とも落ちることを、修正担当と lead の両方が独立に確認した。コード本体は未変更（テスト追加のみ）。

検証ストリームは、修正ストリームに**参加していない**エージェントが担当する
（[`09-review-protocol.md`](09-review-protocol.md) の三段構成）。検証担当が読むのは
指摘リストと修正の差分とテストだけで、修正者の説明は読まない。合格条件は
「指摘がすべて解消 / 新たな問題を入れていない / `./tools/check.sh` が 6/6」の 3 つ。
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

> この DA モデル（`0xD3` / `0xD5` / `0xD9` の状態機械、`erase_range`、パケット会計）は
> 後に `feat/safety-split` で `crates/m241-flash/src/simulator.rs` へ移設された。
> `m241-core` は読み取り専用クレートとして DA を一切持たないため、この節が指す行番号は
> main の時点のものであり、現在の `m241-core/src/protocol/simulator.rs` には存在しない。
> モデルの内容は移設先で上表の変異テストごと維持されている（`tools/check_safety.py` が
> `m241-core` に DA が戻らないことを検査する）。
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

---

## 2026-11-20 / 修正 4 本の独立検証（`fix/fix-*`）

対象: 可読性レビューの Must 8 件と仕様検収の指摘に対する 4 本の修正
（`fix/fix-core` / `fix/fix-gui` / `fix/fix-cli` / `fix/fix-docs`）。
検証は修正に参加していない担当が行い、読んだのは指摘リスト・差分・テストだけ。
修正範囲は `git diff b818676..52f023e`（23 ファイル、+1098/−203）。検証中の
main は `e58c5d3` → `5d6ad74` → `19e07c6` と進んだが、いずれも本節の対象外の
追加（AGENTS.md の追記、simulator の修正）で、4 本の修正内容は変わっていない。

### A. 可読性 Must 8 件

| # | 指摘 | 解消 | 検出するテスト | 根拠 | 判定 |
|---|---|---|---|---|---|
| 1 | `PatchPlan::apply` が `before` を読まない | 解消 | `types::tests::a_plan_whose_before_byte_is_absent_from_the_image_is_refused`（`before` チェックを消す変異で落ちる） | `types.rs:197-225`（判定 `:211`） | 合格 |
| 2 | エコー不一致に偽の `offset: 0` | 一部 | エコー側は `brom::tests::an_echo_mismatch_does_not_claim_to_be_at_offset_zero` が守る。**sync プローブ側は検出テストが無い** | `brom.rs:312`（エコー）、`brom.rs:266`（sync、テスト無し）。指摘が引く `brom.rs:194` は base の sync プローブ側 | **不合格** |
| 3 | `Io.path` が `"<unbound>"` | 解消 | `error::tests::a_bare_io_error_prints_no_placeholder_path`（`From<io::Error>` を `Some("<unbound>")` に戻す変異で落ちる） | `error.rs:72-73` / `:113-116` / `:211-225`、テスト `:411` | 合格 |
| 4 | `damaged_blocks` が同名別物 | 解消 | `types::tests::the_summary_counts_the_damaged_blocks_the_track_listed` | `types.rs:92`（`Vec<u32>`）/ `:131`（`damaged_block_count`）/ `:146` | 合格 |
| 5 | 存在しない `tests/design_invariants.rs` を根拠に | 解消 | 文言の機械検証は無い。参照先は実在する検査に置換 | `theme.rs:7` が `tools/check_design.py` を指す。`design_gate.rs:200` が `check.sh` への配線を固定 | 合格 |
| 6 | `m241-gui` の `m241-core` 未使用依存 | 解消 | 専用テスト無し（`clippy -D warnings` が実質の検出） | `52f023e` で依存を削除、その後 `3393eff` で画面が実際に `m241_core::` を使うため再追加（`screens/common.rs:12`、`connect.rs:15`）。HEAD では未使用でない | 合格 |
| 7 | 「実機で確立した定数」の混在 | 解消 | 文言ポリシーのテストは無し。各定数に由来ラベル | `brom.rs:7-43`（`## Established on hardware` / `## Not established on hardware here`）、各定数 `:63`/`:72`/`:80`/`:89`/`:109`/`:114`/`:120`/`:127`/`:133`/`:141` | 合格 |
| 8 | `frame.rs` の往復テストが恒等写像でも通る | 解消 | `word_reversal_restores_the_vector_table_from_the_wire`、`word_reversal_of_a_single_dword_is_spelled_out`（恒等写像に変異させると 3 件落ちる） | `frame.rs:37-45`（実装）、テスト `:87`/`:105` | 合格 |

### B. 仕様検収の指摘（終了コード 5 の producer）

| 項目 | 確認方法 | 結果 |
|---|---|---|
| 終了コード 5 に producer があるか | `Error::Safety` を追加し、`exit_code_for` に `=> 5`（`main.rs:217`）。`every_documented_exit_code_has_a_producer`（`:448`）、`a_safety_refusal_is_exit_code_five`（`:494`） | 合格 |
| 網羅 match か（`_ => 1` の温存が無いか） | `Error` に `Probe` variant を追加 → core 側 2 か所を直すと **`main.rs:212` の `exit_code_for` が E0004 でコンパイル失敗** | 合格 |
| 未実装エラーの文言が利用者向けか | 実バイナリ実行で `fw flash` / `log geotag` が「このビルドでは `m241 fw flash` は未実装です。`m241 --help` で使えるコマンドを確認してください。」（exit 1）。`--help`/`--version` は 0、不正引数は 2 | 合格（`main.rs:177-182`） |

### C. 新たに入った問題

1. **Must 2 の検出漏れ**（上記 A#2）。sync プローブ側の `offset: None` を `Some(0)` に
   戻す変異が **全 254 テストを通過**する。sync 系テストは
   `sync_accepts_the_four_documented_replies`（`brom.rs:448`）と
   `sync_rejects_a_complemented_reply_it_does_not_know`（`:456`）の 2 件だけで、
   どちらも 2 バイト目以降の不一致（`framing` の分岐）を通らない。
2. **`--quiet` フラグが指摘外で削除されている**。base `main.rs:44-46` の
   `quiet: bool` は未使用（`let _ = (cli.quiet, cli.verbose)`）で、docs にも
   記載が無いため仕様違反ではないが、指摘リストに無い変更。実害は
   `m241 --quiet …` が exit 2 になること。
3. `theme.rs:3` は「Two rules」と書くが、`tools/check_design.py` が機械強制する
   規則は 3 つ（amber の出現位置、角丸トークン、状態色のラベル併記）。
   軽微な文言のずれ。
4. 消されたテストは**無い**（`git diff b818676..52f023e --name-status` に `D` 無し、
   テスト数 58 → 78）。弱められたアサーションも確認範囲では無い。
   `render.rs` の `shapes > 0` は base から据え置きで、テーマ再適用テストは
   逆に強化されている（`render.rs:140` が style/text_styles/panel_fill を比較）。
5. `design_invariants.rs` は base に存在しない（`git cat-file -e
   b818676:crates/m241-gui/tests/design_invariants.rs` が不成立）。指摘 5 は
   `52f023e` 時点では「存在しないファイル」→「別ファイル」の置換に留まったが、
   `3393eff` が実ファイルを追加したため HEAD では解消している。

### D. ゲート

`./tools/check.sh` = **5/5**（HEAD `19e07c6`、fmt / clippy / test / docs / design、
23〜25 秒）。`cargo test --workspace` は **254 件全緑**。

### E. 変異テスト（すべて `git checkout -- <file>` で戻し、都度 `git status` を確認）

| 変異 | 落ちたテスト | 期待どおり |
|---|---|---|
| `reverse_words32` を恒等写像に | `word_reversal_restores_…` ほか frame の 3 件 | はい |
| `PatchPlan::apply` の `before` チェックを削除 | `a_plan_whose_before_byte_is_absent_…` 1 件 | はい |
| `Error::Probe` variant を追加 | core 2 か所を直した後 cli 側が E0004（コンパイル失敗） | はい |
| エコー側 `offset: None` → `Some(0)` | `an_echo_mismatch_does_not_claim_…` 1 件 | はい |
| **sync プローブ側 `offset: None` → `Some(0)`** | **0 件（全 254 緑）** | **いいえ（検出漏れ）** |
| `From<io::Error>` の `path` を `Some("<unbound>")` に | `a_bare_io_error_prints_no_placeholder_path` 1 件 | はい |
| `tools/check_design.py` の `main` を常に 0 に | `design_gate` の 4 件 | はい |
| `theme.rs` の doc のパスを存在しない名前に | 0 件（docs ゲートは Rust ソース内のパスを見ない） | いいえ（指摘 5 の再発は機械検出されない） |

### 判定

**不合格**（`fix/fix-core`）。Must 8 件のうち 7 件は解消し、それぞれ検出テストが
変異で実際に落ちることを確認した。残る 1 件（Must 2）はコードは両方の site で
直っているが、指摘が引用した sync プローブ側に検出テストが無く、退行を止められない。
`fix/fix-gui` / `fix/fix-cli` / `fix/fix-docs` は合格。

---

## 2026-11-21 / 読み取り・書き込みの分離（`feat/safety-split` → `fix/safety-split-v2`）

対象: 読み取り専用クレート `m241-core` と書き込みクレート `m241-flash` の分離および `tools/check_safety.py` ゲート追加。

- **初回検証（`feat/safety-split` `323baa0`、`verify/safety-split` `176e384`）**: **不合格**
  1. `docs/dev/11-safety-review.md` / `AGENTS.md` / `crates/m241-core/src/protocol/mod.rs` の「コンパイルで保証」「持てない」が、`m241-core` 公開 API（`Frame::to_bytes` + `Transport::write_all` + `Port::open` + `pmtk::line`）で反証された。
  2. `tools/check.sh` の `safety` 行を削除しても 5/5 緑になる（配線テスト欠如）。
  3. `crates/m241-flash/src/erase.rs` の消去 ACK 判定が `command == 182` のみに後退し、`$PMTK001,182,5,3` を消去成功として受理。`BackupProof::of_bytes` の doc と実装も矛盾。
  4. `tools/check_safety.py` の穴（`0xD4`・`\xNN`・`m241-core/tests/`・basename 一致 allowlist・`m241-gui` 依存未検査）と行末コメントの過剰検出。
  5. `11-safety-review.md` の未実装項目への `✓` / 存在しない `brom.rs` 呼び出し記述、`attempt_flash` docstring ずれ、`crates/m241-cli/src/main.rs` の重複 doc 行、`crates/m241-core/src/lib.rs` の古い記述。
- **Stage 2 修正（別エージェント、`fix/safety-split-v2` `020ac82`）**: 3 コミット（`2db5ed2`, `fbfa356`, `020ac82`、12 ファイル +731/−115）。
- **Stage 3 独立検証（三人目、修正者の説明・コミット本文は未読）**:

| # | 指摘 | 解消 | 確認箇所・検出テスト |
|---|---|---|---|
| 1 | 過度な「コンパイル時保証」主張 | 解消 | `docs/dev/11-safety-review.md`、`AGENTS.md`、`crates/m241-core/src/protocol/mod.rs` ほかで「書き込み・消去ルーチンを持たない（生トランスポートプリミティブは共有のため公開）」に一貫して限定 |
| 2 | `check_safety.py` 配線・有効性テスト | 解消 | `crates/m241-flash/tests/safety_gate.rs`（12 テスト）。`tools/check.sh` から `safety` 行削除で `the_safety_gate_is_still_wired_into_check_sh` が失敗することを変異テストで実証 |
| 3 | `erase.rs` ACK 後退・doc 矛盾 | 解消 | `crates/m241-flash/src/erase.rs` で `$PMTK001,182,6,<status>` の 3 フィールド＋サブコマンド `6` を厳格検証。変異テストで 2 テスト失敗を実証。`BackupProof::of_bytes` doc 整合 |
| 4 | `check_safety.py` の穴と過剰検出 | 解消 | `0xD4`・`\xNN`・`m241-core/tests/`・完全相対パス allowlist・`m241-gui` 依存検査・文字列認識つき `//` 除去を実装。9 件の変異テストすべて期待どおり（行末コメント `// the erase` は PASS、文字列内 `"//"` 後の違反は FAIL） |
| 5 | 文書・コード不整合と重複行 | 解消 | `11-safety-review.md` の `✓` / 呼出数、`firmware.rs` の `attempt_flash`、`main.rs` 重複行、`m241-core/src/lib.rs` をすべて修正 |

- **ゲート・テスト**: `./tools/check.sh` = **6/6 PASS**、`cargo test --workspace` = **282 passed / 0 failed**（`feat/safety-split` 268 → +14、削除・弱体化 0 件）。個人情報違反なし。
- **判定**: **合格**（`main` へマージ）。

---

## 2026-11-21 / EXIF 読み書きとジオタグ（`feat/exif2` → `fix/exif2-review`）

対象: `crates/m241-core/src/log/exif.rs`、`crates/m241-core/src/log/geotag.rs`、`crates/m241-core/src/log/fixture.rs`、`crates/m241-core/src/log/mod.rs`。

- **Stage 1 発見レビュー（`feat/exif2` `57c1560`）**: **不合格**（Must 2 件・Should 9 件）
  - **Must 1**: `geotag.rs` の `replace_file` が非 Windows でも元ファイル削除フォールバックに入り、さらに `remove_file(path)` 後の再 `rename` 失敗時に唯一残った一時ファイル `&temp` まで削除してしまうデータ消失バグ。
  - **Must 2**: JPEG EOI マーカのリテラル `0xD9`（`exif.rs` / `fixture.rs`）が `tools/check_safety.py` の破壊的オペコード検査に衝突。
  - **Should 3〜11**: `TiffReader::ifd` の `offset < 8` 未拒否、`read_gps_time` の時分秒範囲検証とゼロ分母時の `Err(malformed)` 統一、`read_datetime_original` の `u32` 範囲外エラー化（無言 `mtime` フォールバック防止）、負の高度（M6）と `validate_position`（M8）のテスト追加、`geotag.rs` のテスト名・内容整合と重複 `plan()` 削除、`CaptureTime` の `GeotagPlan` 公開、非 APP1 セグメント長超過エラー文言修正、`ByteOrder` の `pub(crate)` 化、未使用 `GeotagOptions::dry_run` 削除と `GeotagPlan::will_write()` 改名。
- **Stage 2 修正（別エージェント、`fix/exif2-review` `a433142`）**: 4 ファイル変更（+534/−115 行、テスト +8 件追加）。
- **Stage 3 独立検証（三人目、修正者の説明・コミット本文は未読）**:

| # | 指摘 | 解消 | 確認箇所・検出テスト |
|---|---|---|---|
| 1 | `replace_file` データ消失バグ（Must） | 解消 | `crates/m241-core/src/log/geotag.rs` で削除フォールバックを `#[cfg(windows)]` に限定し、`remove_file` 後の再 `rename` 失敗時は `temp` を保持して復旧パスをエラーに含める。3 テスト追加、変異テスト M11 で KILLED |
| 2 | `0xD9` リテラルの `check_safety.py` 衝突（Must） | 解消 | `JPEG_MARKER_SOI: u8 = 0xD8`、`JPEG_MARKER_EOI: u8 = JPEG_MARKER_SOI + 1` に統一。`log/` 配下の `0xD9` 出現 0 件・`tools/check_safety.py` 通過を確認 |
| 3〜5 | `offset < 8`・`read_gps_time`・`read_datetime_original`（Should） | 解消 | `crates/m241-core/src/log/exif.rs` で各境界検査とテストを追加。変異テスト M9・M10・M12 すべて KILLED |
| 6 | 負の高度・`validate_position` テスト（Should） | 解消 | `negative_altitude_round_trips_with_below_sea_level_reference`、`invalid_gps_positions_are_rejected_before_writing` を追加。変異テスト M6・M8 とも KILLED |
| 7〜11 | テスト整合・`CaptureTime`・エラー文言・可視性・API 整理（Should） | 解消 | `exif.rs` / `geotag.rs` / `mod.rs` で全項目解消を diff とテストで確認 |

- **ゲート・テスト**: `./tools/check.sh`（および `tools/check_safety.py`）全ゲート PASS、`crates/m241-core/src/log/` 内テスト **74 件**（`exif.rs` 15 件 + `geotag.rs` 11 件、削除・弱体化 0 件）。本番コードの `unwrap`/`expect`/`panic!`/`unsafe`/無検証添字 0 件、個人情報違反なし。
- **判定**: **合格**（`main` へマージ）。

---

## 2026-11-21 / GUI 設計レビュー修正（`fix/design-review2` → `fix/design-review2-v2`）

対象: `crates/m241-gui/`（レール段差・固定幅、書き込みボタン可読性、ステップマーカー形状区別、1024×640 スクロール・フッター、見本表示、設定画面 5 行、接続画面見出し・数値右揃え、速度チャート最大値位置）。

- **初回 Stage 3 検証（`fix/design-review2` `0321bef`）**: **不合格**（要修正 3 件・推奨整理 3 件）
  1. `crates/m241-gui/src/screens/connect.rs` の `log_panel` のみ `common::figure_line`（右揃え）に変更され、隣の `device_panel` が左揃え `egui::Grid` のまま残存（コメントも事実誤認で並列 2 パネルが不揃い）。
  2. `crates/m241-gui/src/screens/firmware.rs` のステップマーカー塗り／中抜き区別がテストで担保されておらず、`Done` を中抜きに戻す変異テスト M3 が生存。
  3. `crates/m241-gui/src/screens/settings.rs` の UI 文言に内部仕様書名（`DESIGN.md §9`）が露出。
  - 推奨整理: 空ログ時の書き出しフッター非表示化（`log.rs`）、見本モード時の `sample_marker` 重複除去とパネル見出しキャプション統一（`firmware.rs`）、左レール固定幅の回帰テスト追加（M5、`app.rs`）。
- **Stage 2 修正（別エージェント、`fix/design-review2-v2` `1f97b24`）**: 5 ファイル変更（+331/−38 行、テスト +6 件追加）。
- **Stage 3 独立検証（三人目、修正者の説明・コミット本文は未読）**:

| # | 指摘 | 解消 | 確認箇所・検出テスト |
|---|---|---|---|
| 1 | `device_panel` 右揃え統一・コメント修正 | 解消 | `crates/m241-gui/src/screens/connect.rs` で `common::figure_line` に統一。`the_connect_panels_align_their_figures_to_the_right_edge` 追加、変異テスト M1 で FAIL |
| 2 | `StepMarker` 塗り／中抜き単一ソース化・描画検証 | 解消 | `StepMarker::is_filled` 新設、`the_step_markers_are_told_apart_by_shape_not_only_by_colour` 強化 + `the_firmware_step_row_paints_distinct_circle_primitives` 追加。変異テスト M3a・M3b とも FAIL |
| 3 | `settings.rs` 内部仕様書名露出の除去 | 解消 | UI 文言から内部参照を除去し `no_row_exposes_internal_document_names` 追加。変異テスト M_S3 で FAIL |
| 4〜6 | 空ログフッター・見本キャプション・レール固定幅テスト | 解消 | `an_empty_log_omits_the_export_footer_until_a_track_exists`、`the_firmware_sample_renders_one_sample_banner_and_captions_its_panels`、`the_rail_keeps_its_fixed_width_across_every_screen` 追加。変異テスト M_L4・M_F5a・M_F5b・M5 すべて FAIL |

- **ゲート・テスト**: `./tools/check.sh` 全ゲート PASS、`crates/m241-gui/` テスト **93 件**（`main` 79 → `0321bef` 87 → `1f97b24` 93、削除・弱体化 0 件）。本番コードの `unwrap`/`expect`/`panic!`/`unsafe`/無検証添字 0 件、個人情報違反なし。
- **判定**: **合格**（`main` へマージ）。

---

## 2026-11-21 / CLI 8 コマンド配線（`feat/cli-wire2` → `fix/cli-wire2-v2`）

対象: `crates/m241-cli/`（`device ports` / `device info`、`fw dump` / `fw patch`、`log info` / `log download` / `log read`、`nmea repair` の 8 コマンド配線）および `docs/reference/02-cli.md`。

- **Stage 1 並行レビュー（`feat/cli-wire2` `ea58d10`、仕様検収＋可読性レビューを互いの結果を見せずに並行実施）**: **不合格**（統合 Must 8 件・Should 13 件；なお未実装 5 コマンド `device doctor`/`fw flash`/`log erase`/`log geotag`/`nmea proxy` は後続ストリーム対象として分離）
  - **Must 1**: `fw.rs` の `StreamState::Unknown` 拒否（`AGENTS.md` §2.1）が `Error::State`（exit 1）を返しており、かつテストが `without_nmea()`（`Silent`）を渡していたため変異テスト M7 が生存。
  - **Must 2**: `fw.rs` のヘッダ記載サイズ不一致で `declared != len - 4` でも「expected」と表示。
  - **Must 3**: `fw.rs` の `BASE_ADDR_FIRMWARE + offset` が無検査加算。
  - **Must 4**: `nmea.rs` の `repair_zda` に年月日の範囲検証が無く、不正日付を別日付＋有効チェックサムに書き換える。
  - **Must 5**: `logcmd.rs` の `log read --utc-offset-days auto` で `track_days` が「最新」でなく「末尾」を参照し、stale 自動シフトのテスト欠落で変異テスト M6 が生存。また明示 `7168` 指定時も自動検出注記を表示。
  - **Must 6**: `exit.rs` が非フラッシュの `Error::Rejected`（`log download` のデバイス拒否等）を一律 exit 6（フラッシュ書込失敗）に写像（`02-cli.md` は exit 1）。
  - **Must 7**: `fw dump --json`、`log download --json`、`fw patch --json`（適用時）のテスト欠落。
  - **Must 8**: `fw patch` の epoch 定数異常拒否の CLI テスト欠落、および `--dry-run` が既パッチ済みイメージを拒否せず exit 0 を返す不統一。
  - **Should 9〜21**: 内部可視性（`pub(crate)`）と `link::open_pmtk` 統一、`logcmd.rs` モジュール doc・rustdoc リンク、ハードウェア offset 定数名、コメント日付、CH340/CH341 統一、非 TTY `Confirm::pause` の安全拒否、書き出し後パース失敗時の保存先明示、`output.rs` JSON 往復と `progress(_, 0)` テスト、`nmea.rs` 変数名、`dump`/`read_once` 分割、`--length 0`/奇数長の exit 2 化、stdin `"-"` / `--assume-brom` / `Silent` / `--port` / `kml` / `nmea` テスト追加、`02-cli.md` フラグ同期。
- **Stage 2 修正（別エージェント、`fix/cli-wire2-v2` `9e8bf1c`）**: 12 ファイル変更（+1,367/−292 行、CLI テスト 84 → 108 件、+24 件追加）。
- **Stage 3 独立検証（三人目、修正者の説明・コミット本文は未読）**:

| # | 指摘 | 解消 | 確認箇所・検出テスト |
|---|---|---|---|
| 1 | `StreamState::Unknown` 安全拒否（exit 5）とテスト | 解消 | `crates/m241-cli/src/fw.rs` で `Error::Safety` を返却、`dump_refuses_unknown_line_state_with_safety_error` 追加。変異テスト M7a・M7b とも FAIL |
| 2〜3 | ヘッダサイズ警告分離・`checked_add` | 解消 | `report_declared_size` と `checked_add` を実装し単体テスト追加。変異テスト M1 で FAIL |
| 4 | `$GPZDA` 暦妥当性検証 | 解消 | `cal::is_valid_date` による検証と `a_zda_sentence_with_an_out_of_range_date_is_left_alone` 追加。変異テスト M2 で FAIL |
| 5 | `log read --utc-offset-days auto` 最新日時・テスト | 解消 | `track_days` を `.max()` に修正し 3 テスト追加。変異テスト M6a・M6b とも FAIL |
| 6 | デバイス拒否（exit 1）とフラッシュ失敗（exit 6）分離 | 解消 | `CliError::{Core, FlashWriteFailed}` により非フラッシュ `Error::Rejected` を exit 1 に分離しつつ exit 6 の producer を維持。変異テスト M3 で 3 テスト FAIL |
| 7〜8 | `--json` テスト・epoch 異常拒否・`--dry-run` 既パッチ拒否 | 解消 | `--json` 3 テストと epoch 異常拒否テスト追加、`--dry-run` でも既パッチを exit 4 で拒否。変異テスト M4 で FAIL |
| 9〜21 | Should 13 項目（可視性・doc・非 TTY・exit 2・追加テスト・仕様同期） | 解消 | 全 13 項目の解消を diff・テスト・`docs/reference/02-cli.md` で確認 |

- **ゲート・テスト**: `./tools/check.sh` 全ゲート PASS、`m241-cli` テスト **108 件**（84 → 108、削除・弱体化 0 件）。本番コードの `unwrap`/`expect`/`panic!`/`unsafe`/無検証添字 0 件、個人情報違反なし。
- **判定**: **合格**（`main` へマージ）。

## 2026-11-21 / ezTour core の Stage 2 修正（`fix/eztour-core`）

対象: `crates/m241-core/src/log/` の 8 ファイル（coord / edit / exif / export / geotag / import / kmz / mod）。
`feat/eztour-core` `8c2ed2a` に対する仕様検収・可読性レビューの Must を解消した。

- **裁定の記録**: 可読性 Must 1（KMZ の CDATA が `]]>` で壊れる）はリードが事実誤認と確認済み。
  `sanitize_photo_name`（`kmz.rs`）は `]` を `_` に置換するため `]]>` は生成できない。
  修正は行わず、テスト品質の部分（名前経由で CDATA を閉じられないことを固定するテスト）だけを追加した。

| # | 指摘 | 処置 | 判拠 |
|---|---|---|---|
| M1 | KMZ の長辺縮小（`kmzMaxPhotoEdgeSize`）未実装 | **部分的に解消。縮小は未実装のまま明示** | `KmzPhoto::max_edge_pixels` を受ける `jpeg_dimensions`（SOF ヘッダ読取、デコード無し）を追加し、上限超過は `Error::State` で**拒否**（黙って原寸を入れない）。PNG→JPEG に必要なコーデック依存は追加提案のみ。`12-progress-summary.md` §3.2 に明記 |
| M2 | 時刻シフト後の自動再マッチングが無い | 解消 | `geotag::plan_with_shift`（再計画、dry-run 安全）＋ `exif::write_datetime_original`（`DateTimeOriginal` 書き換え）＋ `apply` が同じリライトで時刻と位置を書く。`FileModified` は昇格させない |
| M3 | 距離グループ化が推移的でない | 解消 | union-find（path halving）。A-B 10 m / B-C 10 m / A-C 20 m を 1 グループにし、入力順反転でも同じ分割 |
| M4 | `GPSVersionID` の型・個数不一致が doc 未記載 | 解消 | `GpsTags::version_id` と `read_gps_version` に「型・個数が規格外なら `None`」を明記（`read_coordinate` の慣例に合わせ `Err` にはしない。根拠も doc に記載） |

**仕様検収 Should の処置**: S3/S4（色・`<Point>`）、S30/S31（RMC `V`・GGA 品質 0・RMC→GGA 順）、
S-d（既定値）、S-f（`KmzPhoto::utc` と EXIF の一致、`CaptureTime` の出自を保持）はテスト／実装で解消。
S11/S15（タイムゾーン変換・日時分秒の個別シフト・適用範囲）と S21/S22（非 JPEG の紐づけのみ）は
P1 の範囲外として未実装のまま `12-progress-summary.md` に明記。S-e（GPX 1.1 `<speed>`）は m/s 解釈を doc 化。
C-c（10 色パレット）は単一トラックなので 1 色のまま、理由を定数 doc に記載。

**可読性 Should の処置**: 4（モジュール doc）、5（`last_bucket` 命名＋日付制約）、6（`hemi` 比較）、
7（閉じないコメントを `Error::Log` に）、9（色の読み下しと「ezTour 実出力と未照合」）、10（Snyder §8-9、
`split_track` の複製理由）、11（UTM の範囲 doc）、13（`_original` の命名理由）、14（`civil_from_days` を共用）、
15（`GeotagPlan::keep_backup` の doc）、16（到達不能分岐を除去）、17（`checked_track` に集約）、
18（merge の点順序）、19（`gps_entries_for` 削除）、20（`segment_length_for` に集約）、
21（`crc32_ieee`・`MAX_APP1_PAYLOAD`・`MAX_TIFF_BYTES` を非公開化）、23（`<Point>` と高度既定の doc）、
25（`min(19)` の理由）、C-a（ノルウェー／スバールバル帯の直接テスト）を解消。
2（`<wpt>`）と 3（NMEA の落ちた行数）は doc に設計判断を明記して見送り。

**安全ゲートの指摘**: 新規の JPEG サイズ読取が EOI を生の `0xD9` で書いたため `check_safety.py` が
違反として検出した。`exif::JPEG_MARKER_EOI`（SOI+1）で表すよう修正し、ゲートは一切変更していない。

---

## 2026-11-21 / GUI のログ画面と接続（`feat/gui-eztour-live` → `fix/gui-eztour-live`）

対象: `crates/m241-gui/`（`app.rs`, `screens/connect.rs`, `screens/log.rs`, `screens/settings.rs`, `widgets/rail.rs`, `tests/render.rs`）と `docs/dev/01-design-decisions.ja.md`。
仕様書: `DESIGN.md` §3・§5・§6、`docs/dev/07-eztour-spec.ja.md` §5・§8。実機ポートは開いていない。

- **Stage 1 並行レビュー**（仕様検収＋可読性、互いの結果を見せずに並行実施）: **不合格**。
  変異 13 件中 7 件が生存（M2 `$PMTK182,5` 送信、M3 確認なしジオタグ、M4 無限再生、M6 スライダー無効化、
  M7 速度パネル常時メートル、M8 rail transition 無視、M9 rail の 4px を 0）。仕様違反は
  E5（軌跡が単色・幅 2）、E6（始点／終点マーカー無し）、E4（`fit_all` 後の追従が点を選ばない）。
- **Stage 2 修正（別エージェント）**: 6 ファイル＋設計記録 1 ファイル。
- **Stage 3 独立検証（三人目、修正者の説明・コミット本文は未読）**: **不合格**。Must 1 件（N1、下記の v2 節）。テスト数は 82 + 30 + 6 で、Stage 3 の独立変異 15 件（M7a を除く）のうち **12 件検出・3 件生存**（M2・M3・M13/M14 群）。**Stage 2 追記にあった「変異 8 件中 7 件検出」はこの結果と食い違うため撤回する**（M3 は「M4a は生存、M4c は検出」と書かれていたが、独立変異では末尾の `playing = false` 削除 M3 が生存し、M13（`has_position()` を落とす）・M14（`latitude.is_some()` に変える）も生存した。検出できたのは M4b/M4c 相当のみ）。
  - Must: `GeotagConfirmation(())` が `log.rs` から自由に構築でき、「写真に位置」クリック直後に `apply_geotags(GeotagConfirmation(()))` を足す M2 が全テスト通過（テストが UI 経路を通していなかった）。
  - Should: M3（末尾の repaint 要求が止まらない）、M13/M14（片方だけ座標を持つ点の不変条件が未テスト）、M11b（`app.rs` の rail 配線が未検証）、E6 始点・終点マーカーが無テスト・コメントが 10 色パレットと矛盾、`01-design-decisions.ja.md` の「10 色はトラックごと」の断定が仕様書より強い。


| # | 指摘 | 解消 | 確認箇所・検出テスト |
|---|---|---|---|
| 1 | `probe_client` の送信内容が未固定（M2） | 解消 | `probe_client` を crate 内に落とし、`probe_client_sends_only_read_requests` で `$PMTK605` / `$PMTK182,2,8` / `$PMTK182,2,1` の 3 通だけを assert。変異 M2 で FAIL |
| 2 | ジオタグの確認ゲートが未固定（M3、可読性 Must 1） | 解消 | `GeotagConfirmation` トークン（`confirm_geotag_write` だけが発行）を `apply_geotags` が要求。`asking_to_geotag_changes_no_bytes_until_the_write_is_confirmed` / `confirming_needs_an_open_prompt` を追加。~~変異 M3 で FAIL~~ **Stage 3 の訂正: トークンは `log.rs` 内から構築でき、M2（「写真に位置」直後に書き込む）は生存した。v2 節でトークンを子モジュールへ移し、UI 経路のテストで閉じた** |
| 3 | 再生の自動前進が未検証（M4） | 解消 | `advance_playback(now, interval)` に時計を引数化し、`playback_only_steps_once_the_interval_has_passed` / `playback_stops_at_the_last_point_and_does_not_run_on` / `reduced_motion_slows_playback_to_the_longer_interval` を追加。M4b・M4c で FAIL |
| 4 | スライダーの確定が未検証（M6） | 解消 | `egui::Slider` を自前のトラックに置き換え、押下→ドラッグ→解放を実イベントで流す `dragging_the_playback_slider_moves_the_selected_point` を追加。変異 M6 で FAIL |
| 5 | 軌跡の 10 色・幅 5・不透明度 0.7（E5） | 部分解消＋記録 | 線は幅 5・不透明度 0.7（`theme::metric::track`）、10 色は点の塗りに移した（`theme::TRACK_PALETTE`）。理由は `01-design-decisions.ja.md` §8。`the_track_uses_the_specified_width_and_opacity` / `the_map_track_is_five_points_wide_at_seventy_percent_opacity`。変異 M5 で FAIL |
| 6 | POI（`rcr=0x0008`）のリング表示が未検証（E7） | 解消 | `the_map_draws_a_poi_ring_at_the_waypoint`（描画プリミティブの半径・色）と `a_waypoint_is_drawn_as_a_ring`。変異 M7 で FAIL |
| 7 | `set_follow(true)` が点を選ばず追従が止まる（E4） | 解消 | 追従オンで位置付きの点が無ければ先頭点を選ぶ。`following_picks_the_first_point_that_can_be_plotted` / `following_centres_the_map_on_the_selected_point`（投影座標で中心を assert）。変異 M8 で FAIL |
| 8 | 写真マッチの不変条件・確認件数・`point_index` doc・確認の無効化 | 解消 | `sync_photo_to_selected` と `find_matching_point_index` に不変条件の why、`PhotoEntry::point_index` の doc、確認文はファイル名を列挙（`geotag_targets`）、選択・写真選択・日フィルタ・再読込・トラック差し替えで確認を閉じる（`a_change_that_moves_what_would_be_written_closes_the_prompt`） |
| D2 | rail の reduced motion 配線が未検証（M8 相当） | 解消 | レールの 4px オフセットを `tab_left_at` / `active_step_out` の純関数に切り出し、`the_selected_tab_steps_out_by_four_points_and_the_others_do_not` を追加 |
| D4 | rail の 4px 段差が未検証 | 同上 | 同上（4.0 をリテラルで assert） |

- **見送った Should と理由**
  - 速度パネルの帝国単位の数値突き合わせ（仕様検収 Should 6 / 可読性 31）: `log_screen_renders_playback_follow_mode_and_imperial_units` は `mph` の有無だけを見る。変異 M7 は今回の修正範囲外（`speed_panel` の系列値）なので未解消。
  - OS の reduced motion を読む（Should 4）: 既定の変更は「これまでより動きが減る／増える」の判断を含むため、修正担当の裁量で既定を変えず保留。
  - 上書き拒否の TOCTOU（Should 3）、単位切替の不揃い（2）、`pub` 範囲の残り（1）: 実害の判断が必要で、範囲外の `m241-core` か設計判断に属する。
  - 可読性 Should 11（エラー経路の重複）、27（bounds の 3 重走査）: 挙動を変えない整理で、今回の検証対象外。
- **ゲート・テスト**: `./tools/check.sh` 6/6 PASS。`crates/m241-gui` テスト **112 件**（`ec06f03` の 93 → 112、+19、削除・弱体化 0 件）。
- **変異テスト**: Stage 2 の「8 変異を実行し 7 検出／M4a のみ生存」は、Stage 3 の独立変異（15 件、M7a を除く）で **12 検出・3 生存**（M2 確認なしジオタグ、M3 末尾 repaint、M13・M14 `has_position()`）と判明したため**撤回**する。M4a の生存という判断はそのまま（末尾で `playing = false` を消しても選択位置は止まる）。生存 3 件は v2 節で解消。
- **未検証**: `probe_serial_port` の実機動作、追従の実描画、GUI 経由の実 JPEG 書き込みは 05-verification.md に記録が無く、**未検証**のまま（合格に含めない）。

### Stage 2 v2（別エージェント、`fix/gui-eztour-live-v2`、main `8170c4e` を先にマージ）

変更は `crates/m241-gui/` と `docs/dev/` のみ（`m241-core` / `m241-cli` / `m241-flash` は未変更）。実機ポートは開いていない。

| # | Stage 3 の指摘 | 対応 | 確認箇所・検出テスト・変異 |
|---|---|---|---|
| N1（Must） | `GeotagConfirmation` を `log.rs` 内から構築でき、M2（「写真に位置」直後に書き込み）が全テスト通過 | **解消** | トークンを子モジュール `crates/m241-gui/src/screens/log/geotag_gate.rs` へ移し、フィールドを完全私有（`pub(super)` にすると親の `log.rs` に再び開くため不可）に。`apply_geotags` も画面私有に下げた。唯一の入口は `confirm_geotag_write`（「書き込む」ハンドラのみが呼ぶ）。UI 経路テスト `pressing_geotag_writes_nothing_until_the_write_button_is_pressed`（実クリック → プロンプト表示 → バイト不変 → キャンセル → 再度押下 → 「書き込む」で初めて変化）を追加。M2 の主形（`GeotagConfirmation(())`）は**コンパイル不能**、構築子を使う形は UI テストが **FAIL** |
| N2 | 末尾で `playing = false` を消しても検出されない | **解消（指摘の一部は事実と異なる）** | 末尾の代入自体は `step_next` が既に `playing` を落とすため**もともと冗長**で、削除しても状態は変わらない。実際に有害なのは「`advance_playback_if_due` が終了後も repaint を要求し続ける」形なので、`a_finished_playback_asks_for_no_further_frame`（戻り値）と `a_finished_playback_asks_egui_for_no_further_frame`（`set_repaint_observer` で実フレームの 250/500 ms 要求を数える）を追加。`if true` 変異（終了後も要求する）で **FAIL** |
| N3 | 片方だけ座標を持つ点のテストが無い | **解消** | `the_matched_index_is_always_a_point_with_both_coordinates`（経時点が半分だけ位置を持つ状態を作り、添字が次の実測点へ移ることを要求）と `a_photo_at_a_half_positioned_point_is_not_a_geotag_target`。M13（`has_position()` を落とす）・M14（`latitude.is_some()`）・M14b（`longitude.is_some()`）で **FAIL**。なお写真単位のテストだけでは検出できない（`m241-core` の `geotag::plan` が先に半端な点を拒否するため）ことが分かり、不変条件が効く関数を直接試す形にした |
| N4 | スライダーのキーボード変更・accesskit 値が後退 | **見送り** | 理由は下記 |
| N5 | rail の reduced motion 配線が未検証（M11b） | **解消** | `App::draw` が `rail::rail_transition_secs_for` で求めた値を渡し、`rail_with_transition` が受け取った値を記録する（`#[cfg(test)]`、スレッド単位）。`the_shell_hands_the_reduced_motion_setting_to_the_rail` が実シェルを描いて **0 s / 0.15 s の引数そのもの**を assert。`app.rs` を 0.15 直書きにした M11b で **FAIL**。アニメーション自体も `the_tab_slides_at_the_transition_time_the_caller_passed` が描画位置で測り、M11a・M11c（引数を無視）で **FAIL** |
| N6 | 始点・終点マーカーが無テスト・コメントが 10 色と矛盾 | **解消（削除せず固定）** | コメントをパレットの実態に合わせ、`the_map_marks_the_start_and_the_end_of_the_track_in_ink`（1.5 倍・2.5 倍のリングが ink で描かれる）を追加。削除 → **FAIL**、amber 化 → **FAIL**（design ゲートも検出）。設計判断は `01-design-decisions.ja.md` §8.1 に記録 |
| N7 | `01-design-decisions.ja.md` の「10 色はトラックごと」という断定 | **解消** | 仕様書が明記しているのは「既定色は 10 色パレットの巡回」までであると書き直した |

- **見送った Should と理由**
  - **N4（スライダーのキーボード・accesskit）**: 対象の `playback_controls` は今回まったく変更していない（後退は Stage 2 の変更で入ったもの）。直すには egui の focus と accesskit の値変更を headless で再現する必要があり、テストで固定できない変更は Review の対象外になる。代替経路（`Space`/`←`/`→`、「前の点」「次の点」）は残っているので、必須の「任意位置へ飛ぶ」操作以外は満たす。設計判断として未解決のまま残す。
  - N1 の追加として、パレットの色が**描画**に現れることを `the_map_track_is_five_points_wide_at_seventy_percent_opacity` に追加した（3 種類以上）。色を 1 色に潰す M9 で **FAIL**。
- **ゲート・テスト**: `./tools/check.sh` 6/6 PASS。`crates/m241-gui` テスト **132 件**（lib 88 + render 32 + design_gate 6 + design_invariants 6。Stage 2 の 112 → 132、+20、削除・弱体化 0 件）。
- **変異テスト（v2 で追加）**: M2（コンパイル不能／UI テスト FAIL）、M3 相当（終了後 repaint、FAIL）、M13・M14・M14b（FAIL）、M11a・M11b・M11c（FAIL）、N6a マーカー削除（FAIL）、N6b amber（FAIL）、M9 点の色（FAIL）。**v2 のテストで生き残る変異は 0 件**。ただし末尾の `playing = false` の削除は、`step_next` が同じことをするため**状態に影響しない**（テストでは検出できないが、害もない。冗長な代入として残した）。
- **実機未検証**: Stage 3 と同じ 3 項目（`probe_serial_port`、追従の実描画、GUI 経由の実 JPEG 書き込み）は未検証。

---

## 2026-11-21 / DA 書き込み経路の独立検証 v2（`fix/da-write-g` → `fix/da-write-g-v2`）

対象: `crates/m241-flash/src/da.rs`、`simulator.rs`（+ `lib.rs`、`docs/dev/11-safety-review.md` は前段）。
main `3f72107` を先にマージ済み。実機ポートは開いていない。`FLASH_WRITE_START = 0` と
`check_layout` の拒否は維持し、実機書き込みは解禁していない。

- **Stage 1**（仕様検収＋可読性、`af8bb4e..c9105ed`）: **不合格**。Must 7 件・Should 22 件ほか。
- **Stage 2**（`fix/da-write-g` `142c976`, `aea7923`）: Must と可読性 Must を修正。
- **Stage 3 独立検証**（三人目、`aea7923`）: **不合格**。理由 4 点 —
  (1) `the_stream_stays_aligned_after_a_refused_range` が `0xD3` 拒否後の余分な読み（V11）を検出できない、
  (2) `FakeDa::expected_packets` の `ceil` ルールが V15/V16 で固定されていない、
  (3) `expected_start` のピンが V4（`0x0020_0000` へ変更）で生存、
  (4) Stage 1 Should の見送り理由が未記録。
- **Stage 2 v2**（別エージェント、`fix/da-write-g-v2`、本節）:

| # | Stage 3 の指摘 | 対応 | 確認箇所・検出テスト・変異 |
|---|---|---|---|
| 1（Must） | `0xD3` 拒否後に余分な読みがあっても検出できない（V11 生存、実測 5.0 s） | **解消** | `FakeDa` に `read_bytes()` と `read_polls()` を追加（`simulator.rs`）。`read_byte` は 0 バイト読みを期限まで**ポーリング**するため、余分な読みはバイト数では見えない（空バッファは `Ok(0)`）ので、ポーリング回数で固定する。`the_stream_stays_aligned_after_a_refused_range` を「受理→拒否→再受理」の 3 段に書き換え、拒否コマンドが**ちょうど 1 バイト・ちょうど 1 ポーリング**で終わることを assert。フェイク側の性質は `a_refused_memory_range_leaves_nothing_behind_for_the_next_reply` が単独で固定する。変異 V11（`A5` の後に `read_byte`）で **FAIL**（ポーリング 70 万回超／`left: 701925, right: 1`） |
| 2（Must） | フェイクの `ceil` パケット数ルールがテストで固定されていない（V15/V16 生存） | **解消（1 件は等価と判定）** | 追加: `one_byte_range_with_a_full_size_packet_is_one_packet_not_a_cursor_walk`（範囲 2 バイト・パケット `0x100` → `ceil`=1、余分パケットは `A5` のみで書かれない）、`a_packet_beyond_a_short_range_is_refused_before_it_is_programmed`、`a_packet_past_the_two_packet_count_is_refused`、`a_second_write_command_still_counts_its_own_packets`。**この過程でフェイクの実バグを発見**: `accepted_packets` が `0xD5` ごとにリセットされず、同じフェイクで 2 回目の書き込みが最初のパケットで完了扱いになっていた。`0xD5` 受理時に 0 に戻す修正を入れた。変異 **V15b**（リセット削除）**KILLED**、**旧コード**（`da_cursor > end` かつリセット無し）**KILLED**。**V15 単独**（リセット有り＋`da_cursor > end`）と **V16 単独**（完了前チェック無効化）は**等価変異**として生存（理由は G 節） |
| 3（Must） | `expected_start` のピンが V4 で生存 | **解消** | `check_layout` のローカル `expected_start`/`expected_length` を `EXPECTED_WRITE_START` / `EXPECTED_WRITE_LENGTH`（private 定数）に名前を付け、本体を `check_layout_of(4 引数)` に分離（**公開 API に範囲引数は増やしていない**）。`the_pinned_start_is_the_shipped_start_and_not_the_other_candidate` が「ピン＝出荷値」と「ピンと違う write start は第 3 段で拒否」を assert。変異 **V4** で **FAIL** |
| 4（Must） | Stage 1 Should の見送り理由が未記録 | **解消** | 本節 G に記録 |

- **ゲート・テスト**: `./tools/check.sh` **6/6 PASS**。`cargo test -p m241-flash` = **70（lib）+ 12（safety_gate）= 82 passed**（`aea7923` の 75 → 82、+7、削除・弱体化 0 件）。
- **変異テスト（v2 で実行、すべて HEAD から復元し `git status --porcelain` 空を確認）**:

| ID | 変異 | 結果 | 落ちたテスト |
|---|---|---|---|
| V4 | `EXPECTED_WRITE_START` を `0x0020_0000` に | **KILLED** | `the_pinned_start_is_the_shipped_start_and_not_the_other_candidate` |
| V11 | `0xD3` 拒否 `A5` の後に `read_byte` | **KILLED** | `the_stream_stays_aligned_after_a_refused_range` |
| V15 | 完了判定を `da_cursor > da_range.1` に（リセット有り） | **等価（生存）** | — |
| V15b | `0xD5` 時の `accepted_packets = 0` を削除 | **KILLED** | `a_second_write_command_still_counts_its_own_packets` |
| 旧コード | カーソル判定 ＋ リセット無し | **KILLED** | 同上 |
| V16 | 完了前の余分パケット拒否を無効化 | **等価（生存）** | — |

### G. 見送った Should と理由（Stage 1 由来）

`09-review-protocol.md`「統合」3（Should は直すか、直さないなら理由を残す）に対する記録。
括弧内は Stage 1 の文書と番号（可 = 可読性レビュー、仕 = 仕様検収）。

- **等価変異として記録するもの（コードは変更、テストは追加しない）**
  - **V15（フェイクの完了判定を `da_cursor > end` に戻す）**: パケット長が一定でカーソルが
    各パケットで `length` ずつ増える限り、`ceil((end - start + 1) / length)` と
    「カーソルが初めて `end` を超えたパケット数」は**すべての整数入力で一致する**
    （`start` を 0〜8、`en` を `start`〜`start+19`、`length` ∈ {1..7, 0x10, 0x40, 0x100}
    の全組合せを機械的に照合して 0 件の差異）。差が出るのは `0xD5` ごとにカーソルだけが
    0 に戻り、カウントが戻らない場合だけで、それは上の **V15b** として固定した。
    したがって「フェイクがリセット無しでカーソル判定を使っていた」旧コードは **KILLED**、
    「リセット有りでカーソル判定」は挙動が同一のため生存でよい。
  - **V16（完了前の余分パケット拒否 `>= expected_packets` を無効化）**: 最後の正常パケットで
    `== expected_packets` の枝が `DaStage::Command` へ移るため、次のパケットは
    パケット段ではなくコマンド段で 1 バイトずつ `A5` になる。拒否の**経路**は変わるが、
    観測できる結果（`0x69` も `0x5A` も出さない・書かない・受理数が増えない）は同じ。
    実験で確認: 完了前チェックを無効にした状態で `cargo test -p m241-flash` が全緑（当時の 75 テスト、
    フラッシュも不変）。両方（完了前チェック＋完了時の段移動）を同時に消す変異は **13 件**を落とす
    （KILLED）。よって単独では等価。
- **コードはそのまま、理由のみ記録**
  - **可読性 2（`record_range` と `program` の ACK 読みを `read_ack_pair` に寄せる）**: 両者的に
    似て見えるが、`0xD3` は「1 バイト目が `5A` なら 2 バイト目を読む（`A5` なら読まない）」、
    `0xD5` は「1 バイト目 `5A` の後に `A5` か `5A` かを読む（`0xD5` は必ず 2 バイト）」で、
    **読みの枚数が違い、その違いが安全性そのもの**。1 つの関数に寄せると、この差を
    引数で表すことになり、`0xD3` 側の「読まない」が呼び出し側の選択になる。今回 V11 で
    そこにテストを置いたので、集約は差を隠す方向に働く。見送り。
  - **可読性 3（`Mode` / `Modelled` の改名・統合）**: 挙動に影響せず、`simulator.rs` の
    テスト専用 API の語彙変更。Stage 2 の範囲で全テストを書き換える割に、見つけられる
    誤りが「名前が悪い」以上に無い。別 PR で扱う。
  - **可読性 4（`set_modelled` のリセット対象の明文化）**: `set_modelled` の doc は
    「何を捨て、何を残すか」を列挙済み。残る曖昧さは `da_range`/`da_cursor` を**残す**理由で、
    これは `FakeDa` の doc（「チップの状態を忘れるフェイクは、直前のレンジへ書くバグを隠す」）
    に書かれている。重複を避けるため追記しない。
  - **可読性 19（`0xD5` の消去範囲の citation が 3 か所で食い違う）**: `da.rs` のモジュール doc
    は `0x0010251C`–`0x00102560`（`0xD5` 全体）、定数の doc は `0x0010251C`–`0x00102540`
    （最初のセクタ消去＋失敗分岐）、`check_layout` の doc は全体、と**範囲が違うのは対象が
    違うため**。同じ範囲に揃えると、むしろ失敗分岐（`0x0010253C`–`0x0010254C`）の位置が
    読めなくなる。各所の対象を 1 行で書き分ける案は可読性の向上が小さく、実機未検証の
    アドレス列を触るリスクの方が大きい。見送り。
  - **仕 4（情報レポートが `0x22BA` の後に `0x0022`/`0x00BA` を送る）**: Stage 1 の指摘。
    現在の `da_info_report` は `0x00C2`・`0x22BA`・`0x0000`・`0x0000` を正しく並べており
    （`the_info_report_is_the_measured_twenty_bytes` が先頭 16 バイトを literal で固定）、
    分岐点からのバグは Stage 2 で解消済み。追加作業なし。
  - **仕 5（`0xD3` 単独では消えないことも書く）**: `check_layout` のメッセージは `0xD5` が
    消すと書いており、`0xD3` が記録だけであることは `simulator.rs` のモジュール doc と
    `a_refused_memory_range_keeps_the_range_recorded_before_it` が固定している。エラーメッセージに
    足すと、安全側の警告（0 始まりは危険）が相対化される。見送り。
- **未検証**: `0xD3` の 1 バイト拒否、`0xD9` 無応答、消去失敗の `0xA5`、ゼロ詰めはいずれも
  **実機未検証**（命令列のみ）。`docs/dev/05-verification.md` に記録が無い限り動かさない。

---

## 2026-11-21 / CLI 非破壊 3 コマンド（`fix/cli-non-destructive`）

対象: `crates/m241-cli/`（`cli.rs` / `device.rs` / `lib.rs` / `logcmd.rs` / `nmea.rs` / `testing.rs`）と
`docs/reference/02-cli.md`。`device doctor`（環境・ポート・同定・ログ使用量・drop スキャン・DA 抽出）、
`log geotag`（dry-run / タイムゾーン / シフト / バックアップ）、`nmea proxy`（stdout / TCP、loopback ガード）
を追加した。`fw flash` / `log erase` はスタブのまま。

- **Stage 1 並行レビュー**（仕様検収＋可読性、互いの結果を見せずに並行実施）: 仕様 Must 2 / Should 8、
  可読性 Must 4 / Should 16。
- **Stage 2 修正**（別エージェント、`b82f3ce`）: 上記の Must / Should を解消。
- **Stage 3 独立検証**（三人目、`b82f3ce`）: **合格**。変異 15/15 検出。Should は 19/20 解消
  （R13 のみ部分解消で理由を記録）。
- **Windows CI 修正**（`d1160a1`）: `the_drop_scan_stops_one_level_past_max_drop_depth` が
  `d4/bound.txt` を文字列で比較しており、Windows の `\` で落ちた。`Path::new(path).ends_with(
  Path::new("d4").join("bound.txt"))` に変更。差分はテスト 1 箇所のみで、本番コードは不変。
- **新 head 再レビュー**（`feaaeb6` と `d1160a1`）: **合格**。CLI の 7 ファイルは `b82f3ce` と
  バイト一致で、main 取り込みは機械的。変異 16/17 検出（未検出は `DA_SIZE_BYTES` の定数自体を
  書き換える D1b のみ）。
- **ゲート・テスト**: `./tools/check.sh` 6/6 PASS。`m241-cli` は **131 + 1（`nmea_stdin`）**、
  テストの削除・弱体化 0 件。`fw flash` / `log erase` がスタブであることは
  `the_stub_list_records_how_far_the_wiring_got` が固定している。
- **残した Should / Could（マージを妨げない）**:
  - N1: 誤サイズ DA のテストが期待値を `DA_SIZE_BYTES + 1` から作っており、定数自体の書き換えでは通る。
    リテラル 1 回の assert で塞げる。
  - N2: `--port` 未指定時に probe 行が出ないことの assert が無い。
  - N3: `shift_timestamp_seconds` の到達不能分岐（前回 N1 の継続）。
  - N5: `02-cli.md` の geotag 節に「実カメラ JPEG では未検証」の 1 行が無い。
- **未検証**: 実機の `device doctor` probe、実カメラ JPEG への `log geotag`、Bluetooth SPP の
  `nmea proxy`。いずれも `05-verification.md` に記録が無い限り未検証のまま。
- **記録の補完**: 本節は Stage 3 再レビューの N4（本ラウンドの記録が `10-review-log.md` に無い）を
  埋めるために、マージ後に追記した。


## 2026-11-21 / DA ローダーと `fw probe-da`（`feat/da-loader-probe`）

対象: `git diff 7f520c2..c06bcef`（10 ファイル、+1916 / −13）。DA をチップの RAM へロードする
`DaLoader`（`0xA1` / `0xAF` / `0xA8`）と、読み出し専用 `0xD6` で DA 側のファームウェア基底
（`0` と `0x00200000`）を判定する `m241 fw probe-da`。フラッシュへは書き込まない。

- **Stage 1 並行レビュー**（仕様検収＋可読性、互いの結果を見せずに並行実施）: 仕様 Must 3 / Should 7 / Could 6、
  可読性 Must 5 / Should 12 / Could 7。
- **Stage 2 修正**（別エージェント）: 下の表のとおり Must 8 件をすべて解消。Should 19 件のうち 15 件を解消し、
  4 件を見送った（理由を本節末尾に記録）。

### Stage 1 の指摘（抜粋）

| # | 指摘 | 出典 |
|---|---|---|
| M1 | `fw probe-da` の判定（先頭 4 バイト = `18 F0 9F E5`）は DA ビューで実測がなく、`da.rs` の「`0xD6` 未検証」と `fw.rs` / `02-cli.md` の断定が同居 | 仕様 Must-1 |
| M2 | `base_verdict` が「第 1 候補が未読・第 2 候補が一致」で `Settled` を返す（未読を不一致として扱う） | 仕様 Must-2 |
| M3 | `0xA8` の count が旧実装の `0` でなく `1`。偽 DA もテストも count を見ていない（変異 M1c 未検出） | 仕様 Must-3 |
| M4 | `load` の doc とモジュール doc が「リセットベクタを検査する」と書くが、`check_da_image` はサイズしか見ない | 可読性 M1 |
| M5 | `write_image` 末尾の `let _ = clear_input()` が失敗を捨てる理由を書いていない | 可読性 M2 |
| M6 | `02-cli.md` が「64 バイトを比較」と読み取れる書き方（実際に判定するのは 4 バイト） | 可読性 M3 |
| M7 | `PROBE_PAGE_SIZE` の「偶数」の理由が `0xD6` に当たらない（チェックサムはバイト和） | 可読性 M4 |
| M8 | `Confirmation::PROBE_WORD`（`LOAD`）はどこからも入力されないのに、エラー文が入力を促す | 可読性 M5 / 仕様 Should-5 |

**変異（Stage 1 実測）**: M1/M1b/M2–M8・C3–C5 は検出。**M1c（`0xA8` count）、C1（候補を 1 つに減らす）、
C2（ベクタ比較を 1 バイトに）は未検出**。

### Stage 2 の修正

| # | 処置 | 固定するテスト |
|---|---|---|
| M1 | `BaseVerdict` を `Settled` / `Inconclusive(Inconclusive)` に分け、`Inconclusive` を `Unread` / `NoMatch` / `Ambiguous` に分ける。`Unread` は読めた候補を列挙する。CLI と `--json` は候補ごとに「読めた/読めない」「先頭 4 バイト」「一致/不一致」を出す。doc と `02-cli.md` に「DA ビューでは未検証」を明記 | 判定の純関数: `a_candidate_that_was_not_read_blocks_settling_even_when_the_other_matches`。表示（`print_probe_result` を直接呼ぶ。`probe_da` は通らない）: `probe_da_json_document_names_both_candidates_and_says_no_flash_write`, `probe_da_text_result_shows_read_state_first_bytes_and_the_match_for_both`。`probe_da` 本体を通すのは `probe_da_reads_both_candidates_through_the_fake_and_sends_no_write_opcode` |
| M2 | `Settled` は「2 候補とも読めて、ちょうど片方が一致」のときだけ | 同上 + `the_probe_verdict_settles_only_on_exactly_one_vector_match` |
| M3 | `0xA8` を `Frame { count: DA_JUMP_COUNT = 0 }` で構築（旧実装と同じ zero）。偽 DA が `count != 0` を数え、テストで固定。`03-hardware-facts.md` と `01-da-protocol.md` §3.2 に根拠と未検証を記載。ロード範囲と BootROM スタックの非重複を区間で固定 | `the_jump_frame_carries_count_zero_and_the_fake_refuses_any_other`, `the_load_range_does_not_touch_the_bootrom_stack` |
| M4 | 選択肢 (a): `DA_IMAGE_HEAD = [00, 00, A0, E1]`（ベンダー `DA_v3.8.4.bin` の先頭 4 バイト = `mov r0, r0`）を追加し、`check_da_image` がサイズの次に比較。DA バイナリはコミットしない | `the_loader_refuses_an_image_whose_first_bytes_are_not_the_da`, `the_loader_constants_are_pinned_to_the_documents_and_the_read_crate` |
| M5 | 失敗を捨てず `Result` を返す。理由（ずれた echo は次の `read_back` の 9 バイト echo 比較で `Framing` になる）をコメントに書く。ポーリング抽象は持たないので、1 回の drain 失敗はそのままエラーになる | `a_drain_that_fails_is_reported_instead_of_a_successful_load`（Stage 3 の Should-2 で追加。drain の `Io` が `load` のエラーになり、`0xAF` の読み戻しと `0xA8` に到達しないことを固定）。成功経路は `the_loader_writes_the_whole_image_to_the_da_base_and_starts_it` |
| M6 | `02-cli.md` を「判定は先頭 4 バイトだけ、残りは表示用」に書き直し、実機ログとして貼れる出力例も追記 | — |
| M7 | `PROBE_PAGE_SIZE` の doc を「偶数は `0xD6` の要件ではなく、ページ長は DA が受け付けるか未検証」に修正 | — |
| M8 | `PROBE_WORD` を削除し、`of_probe_command_line` のエラーを `--yes` のみに。`11-safety-review.md` の記述も追随 | `the_loader_refuses_without_a_confirmation_and_sends_nothing`, `probe_da_refuses_without_yes_before_opening_a_port_or_reading_a_file` |

**追加で解消した Should**: C1 / C2 の検出（`the_probe_path_reads_both_candidates_and_nothing_else`、
`the_probe_verdict_compares_all_four_vector_bytes`）、候補一覧の検証（`base_verdict` が 2 つ以外を拒否）、
`read_candidates` を `m241-flash` に置いて CLI からアドレスを消した（Should-7 / `11-safety-review.md` の
「任意アドレスを受け付けない」を実装に合わせた）。

**偽 DA の `0xD6` に見つかった既存の欠陥（本節で修正）**: c06bcef の `send_page` はページの開始位置を
`read_page × page_len` として範囲の先頭からのオフセットで数え、`begin_read_range` は固定長のページだけを
用意していたため、**開始アドレスが `0` 以外の読み出しに、要求した範囲と違うバイト列を返していた**
（`0x100` を読むと `0x00` の内容）。`read_returns_the_window_at_the_address_asked_for` はデータを窓の
`0x100` に置いていたので、偽 DA 側のずれとテストの前提が同じだけずれて一致していた。範囲全体を
`start` からコピーし、ページをその中の位置で切り出し、ページ数を §5.5 の `udiv(total+page-1, page)` に
合わせた。固定するテストは `the_fake_pads_a_partial_page_by_default_and_can_send_it_short`
（Stage 3 の Should-3 で `a_two_page_read_is_the_same_bytes_however_the_da_pads_the_range` から
改名・強化した。旧テストは範囲長がページ長の倍数で、名前に反して短いページを作っていなかった。
`read_range` 自体がページ長の倍数以外を `Error::State` で拒否するため、短いページはクライアントからは
到達しない。テストが固定するのは偽 DA のページ詰めのモデルのほうで、それをテスト名と本文で明示した）。
併せて `05-verification.md` に `0xD6` / DA ビューのベクタ / V1.14 個体で probe を走らせていないことを追記した。

**見送った Should と理由**

- Should-2 の「最終ページの advance を送らない」側への寄せ: 送る／送らないは実機でのみ決まる。§5.5 の
  「Nothing is sent when the range is complete.」は DA の返信についての文であり、host 側の advance を
  省略してよいとは書いていない。テストとシミュレータは「最終ページ後も 1 バイト送り、DA は何も返さない」
  で一致させ、未検証点として `05-verification.md` に新設の行で記録した。片方に寄せる根拠が無い。
- Should-1 の「クライアント側が短いページを受ける」側: `PROBE_PAGE_SIZE` は 64 で固定なので、判定が
  必要とする最初のページが短くなる経路は無い。範囲全体がページ長の倍数でない場合の最終ページは
  クライアントが要求しない。偽 DA に `without_page_padding` を足して「ページ長の倍数でない範囲」でも
  同じバイト列が返ることは固定した。
- Could-2（`0x7D` 待ちの境界）・Could-3（`PROBE_PAGE_SIZE` を可変にする）は実機の値が要る。片付けず、
  `05-verification.md` に未検証として残した。
- Could-5（偽 DA の `0xA1` アドレス未検査）と Could-6（`03-hardware-facts.md` の追記）は解消済み。
  Could-4（テスト名）は **当時の記録が誤り**。Stage 2 では改名されておらず、
  `a_da_that_did_not_start_is_a_framing_error_rather_than_a_wait` が `Error::Timeout` を期待したまま
  残っていた（Stage 3 が指摘）。Stage 3 の Should-6 で
  `a_silent_port_after_the_jump_is_a_timeout_not_a_framing_error` に改名し、`Framing` 側は
  `bytes_that_are_not_a_report_end_the_scan_as_a_framing_error` として別に固定した。

**訂正（Stage 3 の Should-1）**

- Stage 2 の報告には「`&[u8]` の `get(24..26)` が `None` を返すので `Vec<u8>` でも同じはず」という
  主張が混ざっていたが、これは誤り。`Vec<u8>` も `&[u8]` も範囲内なら `get(24..26)` は `Some` を返す。
  HEAD のコードに `get(24..26)` は無く（`first_bytes` は `page.get(..VECTOR.len()).unwrap_or(page)`）、
  この誤った前提に基づくコードは残っていない。以下の記述は環境依存の主張なので取り下げる。

### ゲート

`./tools/check.sh` 6/6、`cargo test -p m241-flash -p m241-cli`。Gate の詳細は Stage 3 の節（未検証）。

### Stage 2 v2（Stage 3 の Must / Should への対応）

Stage 3（`/tmp/opencode/verify-da-loader-probe.md`）の **V-Must-1** と Should 1〜6 に対応した。

| # | 処置 | 固定するテスト |
|---|---|---|
| V-Must-1 | `m241-cli` のテスト用の偽 DA（`crates/m241-cli/src/testing.rs` の `fake_da` モジュール）を追加し、`probe_da` を**最後まで**駆動する。偽 DA は BootROM の sync、NMEA ストリームと `$PMTK180`、`0xA1` / `0xAF` / `0xA8`、DA のバナー、`0xD6` のページ会話を最小限に模し、受けた opcode・`0xA1` のアドレス・`0xA8` の count・`0xD6` の範囲を記録する | `probe_da_reads_both_candidates_through_the_fake_and_sends_no_write_opcode`, `probe_da_with_the_vector_at_the_first_candidate_settles_on_it`, `probe_da_uses_silence_and_assume_brom_without_touching_the_reset_lines` |
| Should-1 | 上記「訂正」のとおり、`get(24..26)` の誤った主張を取り下げた | — |
| Should-2 | 偽 DA に `failing_clear_input()` を足し、drain 失敗が `load` の `Error::Io` になり、読み戻しと jump に到達しないことを固定 | `a_drain_that_fails_is_reported_instead_of_a_successful_load` |
| Should-3 | 旧テストを改名・強化。短いページは `read_range` からは到達しない（ページ長の倍数以外は `Error::State`）ため、「偽 DA のページ詰めのモデル」を固定するテストだと名前に明記 | `the_fake_pads_a_partial_page_by_default_and_can_send_it_short` |
| Should-4 | 上の M1 / M2 / M5 の「固定するテスト」列を、実際に通る関数とテスト名に直した | — |
| Should-5 | `Frame` の doc に「count は型では守れず、`DA_JUMP_COUNT` との一致はテストで固定する」と書いた | `the_jump_frame_carries_count_zero_and_the_fake_refuses_any_other` |
| Should-6 | テスト名を期待値に合わせて改名し、`Framing` 側は別テストにした | `a_silent_port_after_the_jump_is_a_timeout_not_a_framing_error`, `bytes_that_are_not_a_report_end_the_scan_as_a_framing_error` |

**変異（実測、各変異のあと元に戻して `git status --porcelain` 空を確認）**: M9（`probe_da` に `0xD5`）は
`probe_da*` の 3 本で検出。M2c（2 候補目を捨てる）は 2 本で検出。`DA_JUMP_COUNT = 1`、ベクタ比較 1 バイト、
未読 → `Settled`、先頭 4 バイト検査の除去もすべて検出。

### Stage 3 検証

未検証（別エージェントが記入する）。

---

## 2026-11-22 / PR #19 `fix/flash-d3-resync` の Stage 2 修正

対象: `git diff origin/main...ad489d5`（9 ファイル、+715 / −39）。Stage 1 は仕様検収
（[仕様レビュー](https://github.com/haruki0420/holux-m241-rs/pull/19#issuecomment-7075378104)）と
可読性（[可読性レビュー](https://github.com/haruki0420/holux-m241-rs/pull/19#issuecomment-7075374592)）を
別々のレビュアーが並行実施。Stage 2 は別エージェントが担当し、Stage 3 で検証する。
Stage 1 の 2 レビュアーは `./tools/check.sh` を実行していない（ビルド成果物を避けるため）。

### Stage 1 の Must への対応

| # | 指摘 | 対応 | 固定するテスト |
|---|---|---|---|
| 仕様 M1 | `02-cli.md` の `da-restart` 節に実機未検証の注記が無い | 節内に「⚠️ 未検証 on hardware」と `docs/dev/05-verification.md` §2 への参照を追加。出力例も「再起動は確認できない」に更新 | — |
| 仕様 M2 | 再試行前の再入失敗に「The write itself finished」「`--recover`」の誤案内が付く | `reenter_bootrom` に `Reentry::{ReadBack, BeforeRetry}` を追加し、エラー文を分岐。再試行前は「何も書いていない・`fw dump` で確認」に。`RESTARTED_NOT_RETRIED_NOTE` を新設し `--recover` を出さない | `flash_retry_reentry_failure_does_not_claim_a_write_finished` |
| 仕様 M3 | `0xD3` 成功・`0xD5` 未送信の境界テストが無い | fake に `failing_write_command()`（`0xD5` ヘッダの送信を失敗させる）を追加し、再試行しないことを固定。`program_started` はヘッダ送信**前**に立つ | `flash_does_not_retry_when_the_0xd5_header_cannot_be_sent`, `a_failed_d5_header_write_still_counts_as_program_started` |
| 可読性 M1 | `flash` に復旧が 4 段の入れ子で埋まり、`note_write_failure` + `with_note` が 6 箇所重複 | `recover_failed_write` と `WriteAttempt` に切り出し、`write_failure` ヘルパ 1 つに集約。`flash` は「試行 → 復旧/再試行 → 読み戻し」の一直線に | 既存の retry / no-retry テスト一式 |
| 可読性 M2 | 再同期が残留 `0x5A` を ACK と誤認しうる点の説明が無い | `read_d3_reply` を切り出し、status `0x00` 検査が第二の防御であること、観測残留は `0xBB` 4 バイト、`5A 00` の場合は `0xD5` の最初のパケット前に止まることを rustdoc に記載 | `a_residue_five_a_is_only_accepted_with_the_zero_status` |
| 可読性 M3 | 「再起動した」と断定する文言（`0xD9` は応答を読まない） | `RESTARTED_*_NOTE` 3 定数と `da_restart` の結果文を「`0xD9` を送った／再起動は確認できない／`fw dump` で確認」に変更。`02-cli.md` も追随 | `da_restart_sends_one_d9_without_reading_a_reply_or_touching_the_control_lines`（期待文字列を更新） |

### Stage 1 の変異テストで生存した 3 件

| # | 生存した変異 | 対応 | 固定するテスト |
|---|---|---|---|
| MUT1 | `0xD3` 再同期のスキップ上限の境界が無い | 8 バイトの残留は成功し、9 バイト目は `after skipping 8 non-reply byte(s)` で拒否することを固定 | `the_d3_resync_accepts_eight_bytes_and_refuses_the_ninth` |
| MUT2 | `0xD3` status 非 0 の分岐が未実行 | fake に `set_d3_status` を追加し、`0xD3 status byte was 0x01` で停止・`0xD5` を送らないことを固定 | `a_non_zero_d3_status_is_a_framing_error_and_nothing_is_written` |
| MUT3 | `check_layout_of` のピン検査が後段エラーと区別されない | ピンだけ動かしたとき、後段の `write_start != expected_start` ではなくピン固有の文言で失敗することを固定 | `moving_only_the_pinned_start_is_refused_by_the_pin_rule` |

変異の実測: ピン検査の削除、`skipped >= LIMIT` → `>`、status 検査の削除の 3 つを入れ、
対応するテストがそれぞれ落ちることを確認した（各変異のあと元に戻し `git status` で確認）。
残り 2 件の生存変異（等価変異・低リスク）は指示どおり対応しない。

### Should への対応

| # | 指摘 | 対応 |
|---|---|---|
| 仕様 S1 / 可読性 M2 | 再同期が `0x5A` を含む残留を ACK と誤認しうる | 可読性 M2 と同じ。`5A 01` の残留を拒否するテストを追加し、設計で消せない `5A 00` は rustdoc に残余リスクとして明記（観測残留は `0xBB`） |
| 仕様 S2 | 再試行前の `verify_device` 省略の根拠が無い | `reload_for_retry` の rustdoc と `02-cli.md` に「`0xD3` はフラッシュを触らないので、`0xD5` 前の失敗はフラッシュを変えていない」と明記 |
| 仕様 S3 | 再入路のテストが `0xA8` 回数と `opens() > 0` だけ | fake の `$PMTK180` を `pmtk180_requests` として回数化し、再試行の再入で 1 回（2 回失敗のケース）、読み戻し込みで 2 回（成功ケース）を固定 |
| 仕様 S4 / 可読性 S4 | `with_note` の doc と実装が矛盾 | doc を「`Framing` / `Timeout` / `Rejected` / `Safety` は variant を保ち、それ以外は `State`」に修正 |
| 仕様 S5 | `02-cli.md` の 🧪 に自動リトライが列挙されていない | 🧪 注記に「自動 `0xD9` と 1 回再試行」「`0xD3` 応答の再同期」を追加 |
| 仕様 S6 | `D3_RESYNC_LIMIT = 8` の根拠が citation でない | doc に「観測は 4 バイト（`05-verification.md` §1.3）、8 は余裕で protocol 値ではない」と明記 |
| 可読性 S5 | `range_attempted = false` の dead store | 削除。`program_started = false` は試行開始時のリセットとして残す |
| 可読性 S6 | `program_started` の「送信前・失敗時も true」の理由が無い | フィールド・accessor・設定箇所に「送信前に立てる。失敗した書き込みは未送信を証明できない」と明記 |
| 可読性 S7 | モジュール doc のコマンド列挙に `flash` が無い | 列挙と説明に `flash` を追加 |
| 可読性 S8 | `flash` の rustdoc 手順に自動復旧が無い | 手順 5 として自動復旧を追加し、読み戻しを 6 に繰り下げ |
| 可読性 S9 | `assert!(ports.opens() > 0)` が主題を検証しない | `assert_eq!(ports.opens(), 1, ...)`（再試行は同じ transport で再入し、ポートを開き直さない）に変更 |
| 可読性 S10 | `0xD3` 拒否の 2 機構（`refuse_ranges` と `refuse_ranges_remaining`） | カウンタ 1 本に統一し、`usize::MAX` を「ずっと拒否」に |

### Could

- 対応: 11（`write_once` → `WriteAttempt::write`。vendor 時に `image` を使わない旨を doc に記載）、
  12（エラー文を `non-reply byte(s)` に）、13（`record_range` から `read_d3_reply` を切り出し）、
  14（冗長な `assert!(chip.restarted())` を削除）。
- 見送り: 15（fake の `0xD3` アーム分割）はテスト専用 fake の整理で、別途まとめる。
  Could 2（`12-progress-summary.md` の `da-restart` 注記）と Could 3（拒否メッセージの
  「keeps whatever ranges…」）は、後者は仕様検収の項目 4（`A5` を「空」と扱わない）が
  明示を求めている文面でもあるため、現状のまま残す。

### ゲート

`./tools/check.sh` 6/6（`M241_VENDOR_IMAGE` を設定した状態。実機ポートは開かず、
`m241-core` と `tools/` は未変更）。

## 2026-11-22 / PR #20 `feat-log-kmz` の Stage 2 修正

対象: `git diff origin/main...2e7468c`（6 ファイル、+764 / −12）。Stage 1 は仕様検収
（[仕様レビュー](https://github.com/haruki0420/holux-m241-rs/pull/20#issuecomment-7078584766)）と
可読性（[可読性レビュー](https://github.com/haruki0420/holux-m241-rs/pull/20#issuecomment-7078580687)）を
別々のレビュアーが並行実施。Stage 2 は別エージェントが担当し、Stage 3 で検証する。
変更範囲は `crates/m241-cli` と docs のみ（`m241-core` / `m241-flash` / `m241-gui` /
`Cargo.*` / `tools/` は未変更）。Stage 1 の 2 レビュアーは `./tools/check.sh` を
実行していない。

### Stage 1 の Must への対応

| # | 指摘 | 対応 | 固定するテスト |
|---|---|---|---|
| 可読性 M1 / 仕様 S1 | `log kmz` の写真スキップ警告が理由を「gap 超過」と断定し、非 JPEG・撮影時刻なし・EXIF 破損でも誤った次の行動を促す | `GeotagPlan::skipped` の理由をそのまま出す。実体化を `materialize_kmz_photos` に切り出し | `kmz_reports_the_planners_reason_for_a_photo_it_leaves_out` |
| 仕様 M1 | `log kmz` の `--utc-offset-days` がマッチング前に適用されることが無テスト | `apply_utc_offset` を `geotag::plan` の前に置いたまま、WNRO ずれログ＋実時刻の写真が同梱されることを固定 | `kmz_applies_the_utc_offset_before_matching_photos` |
| 仕様 M2 | `log edit` が入力ダンプを変更しないことが無テスト | 分割・結合の実行前後で入力（分割元と `--merge` 入力）のバイト列が不変であることを固定 | `edit_does_not_modify_the_input_dump` |
| 仕様 M3 | merge が時刻なしダンプを最後に置く分岐が無テスト | 時刻あり 2 本＋時刻なし 1 本を join し、JSON の `utc` が `[Some, Some, None]` の順になることを固定（`--merge` 2 回指定も同時に固定） | `edit_merge_puts_a_dump_without_timestamps_last` |

### Should への対応

| # | 指摘 | 対応 |
|---|---|---|
| 可読性 S1 / 仕様 S3 | `edit` だけが `ParseReport` を捨て、破損ブロックの警告を出さない | `warn_about_damage` を本体ダンプと各 `--merge` 入力で呼ぶ。merge 入力は複数あり得るので警告に path を付ける |
| 可読性 S2 | `edit` の `# Errors` が `Error::State`・split の `Error::Log`・sibling の `Error::Safety` を網羅しない | rustdoc に 4 種を明記 |
| 可読性 S3 | delete のエラーが exclusive 表記（`0..N-1`）で、0 点時に「0 番が有効」と読める | `0..=N-1` の inclusive 表記にし、0 点は「the log has no points」 |
| 可読性 S4 | `edit` が 44 行で抽象度が混在 | `write_edited` に書き込み＋報告を切り出し、`edit` は操作の選択と実行だけに |
| 可読性 S5 | `kmz` が 38 行で抽象度が混在 | `plan_kmz_photos` と `materialize_kmz_photos` に切り出し、`kmz` は読み込み→計画→実体化→書き出しの一直線に |
| 可読性 S6 | 「merge が空だから delete」という暗黙推論 | `LogEdit` enum と `requested_edit` で操作を明示。track index `0` は `FIRST_TRACK` に命名 |
| 可読性 S7 | モジュール doc 冒頭 2 行がコマンド列挙 | 目的文「Convert, edit and bundle a downloaded track log.」を先頭に |
| 可読性 S8 | CLI の KMZ doc に外部ビューア未検証の注記が無い | `kmz` の rustdoc に「Google Earth と `unzip` は未実行」を明記（`02-cli.md` は既に明記済み） |
| 可読性 S9 | `--max-photo-edge` のテストが core のメッセージ文字列に依存 | 見送り（下記「見送った Should と理由」） |
| 仕様 S2 | `log kmz` に `--tz-offset-minutes` が無く、ローカル時刻カメラでマッチしない | kmz に `--tz-offset-minutes`（既定 0）を追加。カメラ時計に移したコピーでマッチし、アーカイブと写真の時刻は UTC に戻す。`02-cli.md` を追随 |
| 仕様 S4 | split の派生 2 出力目が既存ファイルを無確認で上書き | `fileio::ensure_free` で両半分を書く前に拒否（exit 5）。`02-cli.md` に「先に削除・改名」を明記 |
| 仕様 S5 | 負のテスト不足（`edit --utc-offset-days` の exit 2、`--split-at 0`・点数 の exit 1、2 操作の exit 2） | `log_edit_requires_exactly_one_operation` を拡張し、`edit_refuses_a_split_that_would_leave_an_empty_half` を追加 |
| 仕様 S6 | `12-progress-summary.md` が CLI 15 コマンド・`kmzMaxPhotoEdgeSize` CLI 未接続のまま | 17 コマンド、kmz の CLI 経路・`log edit` を追記。テスト件数を実測値に更新 |

### Could

- 対応: 可読性 C1（`count_label` に統合）、C2（`b"photo/".len()`）、C3（`--merge` の
  help を「フラグを繰り返す」に）、C4（split 2 本目の書き込み失敗時に「1 本目は
  書かれた」を stderr に）、C5（kmz の `# Errors` に ZIP32 超過の `Error::Log`）、
  C7（`second_sibling` に固定）、C8（`edit` が `--utc-offset-days` を受け付けないことを
  テストで固定）。仕様 C1（`-o` 必須の負テスト）、C2（photos 省略時の実行テスト）、
  C3（上限以内の byte-for-byte を CLI 経由で）、C4（`GeotagOptions.max_photo_edge_pixels`
  の冗長設定を削除）、C6（merge の doc 表記）、C7（拡張子なしの `-2` 命名を doc に）、
  C8（2 操作の exit 2 を直接断言）。
- 見送り: 仕様 C5（写真の二重読み）は `KmzPhoto` が `jpeg_bytes` を呼び出し側から
  受け取る crate 契約の帰結で、直すには core API の変更が要る（本ラウンドの変更範囲外）。

### 見送った Should と理由

| # | 指摘 | 理由 |
|---|---|---|
| 可読性 S9 | `the_photo_edge_limit_reaches_the_kmz_exporter` が core の文言に結合 | 実際に縮小されることは `m241-core` の `kmz` テスト（縮小・再エンコード・上限以内 byte-for-byte）が固定しており、この CLI テストは「`--max-photo-edge` が exporter に届いた」配線だけを固定する正直なテスト。実 JPEG を encode して CLI 側でも縮小を測るには `image` を `m241-cli` の dev-dependency に足す必要があり、`Cargo.lock` の変更が本ラウンドの変更範囲（`crates/m241-cli` と docs）を超える。上限以内の byte-for-byte は CLI 経由のテストを追加済みで、文言依存はこの 1 本に残る。 |
| 可読性 C6 | core の split メッセージ「must be 1..{len}」 | `m241-core/src/log/edit.rs` の文言で、本ラウンドは `m241-core` の挙動・文言を変えない条件。CLI 側は split の失敗を exit 1 として固定した。 |

### ゲート

`./tools/check.sh` 6/6（`M241_VENDOR_IMAGE` を設定した状態。実機ポートは開かず、
`m241-core` / `m241-flash` / `m241-gui` / `Cargo.*` / `tools/` は未変更）。


---

## 2026-11-22 / PR #21 `feat-log-edit` の Stage 2 修正

対象: Stage 1 head `660d9a4`（base `origin/main` = `3202c34`）。Stage 1 は仕様検収
（[仕様レビュー](https://github.com/haruki0420/holux-m241-rs/pull/21#issuecomment-7078597563)）と
可読性（[可読性レビュー](https://github.com/haruki0420/holux-m241-rs/pull/21#issuecomment-7078619948)）を
別々のレビュアーが並行実施した。Stage 2 は別エージェントが担当し、Stage 3 で検証する。
Stage 2 の差分は本節と同じコミットで、範囲は `crates/m241-core/src/log/` と `docs/` のみ
（`m241-cli` は別 PR が担当するため触っていない）。

### Stage 1 の Must への対応

| # | 指摘 | 対応 | 固定するテスト |
|---|---|---|---|
| 可読性 M1 | モジュール doc の "PhotoTimeShift is the first / PhotoShiftScope the second" が直前の 3 スコープ列挙に係り、`PhotoTimeShift = allPhotos` / `PhotoShiftScope = selPhotos` と誤読させる | 2 入力を名詞で書き分けた。「シフトの大きさ（日/時/分/秒）」と「適用範囲（3 スコープ）」が別の入力であることを明示し、`byCamera` を含む 3 スコープが後者に属すると読める形にした | —（doc のみ） |
| 可読性 M2 | `CameraModel` の doc は「末尾の NUL/空白だけ除去して byte 比較」だが、実装は `from_utf8_lossy` + `.trim()`。非 UTF-8 の異なる名前が同じ `String` になり、先頭空白も落ちる | **実装を安全側に寄せた**。`str::from_utf8` で厳密に読み、末尾の NUL/空白だけを除去して完全一致で比較する。非 UTF-8 のタグは `None`（不明扱い）とし、lossy 変換による誤同一視をしない。doc も同じ規則に書き換え、`06-open-questions.md` §6 に未確定として記録 | `camera_names_that_are_not_valid_utf8_are_unreadable_not_collapsed`, `leading_whitespace_is_part_of_the_camera_name`, `trailing_nul_and_space_padding_is_trimmed_from_camera_names` |
| 可読性 M3 | 「a value longer than four bytes lives at an offset from the header, which is the layout cameras use」は実カメラ未検証なのに断定 | 「この walk が受け付ける配置」に弱め、実カメラでの確認は未検証と明記。モジュール doc の「Not verified on camera files」にも IFD0 配置を追加し、`12-progress-summary.md` と `06-open-questions.md` §6 に未検証として記録 | —（doc のみ） |

### 仕様検収の Should への対応

| # | 指摘 | 対応 | 固定するテスト |
|---|---|---|---|
| S1 | scoped shift の end-to-end テストが無い（選択写真だけが書き換わることを直接固定していない） | `SelectedPhotos` と `ByCamera` の両スコープで plan → apply し、選択写真の `DateTimeOriginal` がシフト後になり、非選択写真はバイト列も時刻も変わらないことを固定 | `a_scoped_shift_writes_the_clock_into_the_scoped_photos_only` |
| S2 | icon 経路で「フレームヘッダ不明はバイト列そのまま」の分岐が未テスト | SOF を持たない合成 JPEG で `export_kmz_with_icons` を呼び、`photo/` と `icon/` の両方が元バイト列になることを固定 | `a_photo_with_no_readable_frame_header_keeps_its_bytes_under_icon` |
| S3 | 新しい未確定点が `06-open-questions.md` に無い（`AGENTS.md` §7 の置き場は 06） | §6（byCamera / selPhotos の一致規則、日=24h、実カメラ IFD0 配置）と §7（`icon/` の寸法・命名・KML 参照、ezTour が実際に置いていたか）を追記 | —（docs） |
| S4 | 新 API が `log` 直下に再エクスポートされていない | `PhotoTimeShift` / `PhotoShiftScope` / `CameraModel` / `plan_with_time_shift` / `KmzIconOptions` / `export_kmz_with_icons` を `log/mod.rs` に追加（既存 re-export は変更なし） | —（コンパイルで担保） |
| S5 | 既定 scope = `AllPhotos` のテストも未確定記録も無い | 既定をテストで固定（仕様が既定を規定しない実装判断である旨をテストのコメントに記載） | `the_default_scope_is_every_photo` |

### 可読性レビューの Should への対応

| # | 指摘 | 対応 | 固定するテスト |
|---|---|---|---|
| S1 | overflow テストが「写真を読む前に拒否」を名乗りながら、読める写真を渡すだけで順序を検証していない | 存在しないパスを渡し、`Error::Io` ではなく `Error::State` になることを固定。テスト名も `..._before_any_photo_is_read` に改名 | `a_shift_that_overflows_seconds_is_refused_before_any_photo_is_read` |
| S2 | JPEG セグメント走査が 3 実装目。`0xD0..=JPEG_MARKER_SOI` は `exif.rs` の綴りと違い、SOI の値に暗黙依存 | 推奨どおり `exif` に `pub(crate) exif_tiff_block` を追加し、既存の `find_exif_app1` を再利用。geotag 側の走査を削除し、malformed を `None` に落とす差分だけ呼び出し側で吸収 | 既存の camera テスト一式（`the_camera_model_is_read_from_ifd0_make_and_model_in_both_byte_orders` ほか） |
| S3 | `ifd0_ascii` が 40 行・4 抽象度で、`try_into().ok()?` の連鎖が TIFF の配置を隠す | `read_u16_at` / `read_u32_at` を追加し、文字列正規化を `camera_string` に分離。IFD エントリの配置をコメントで明示 | camera テスト一式 |
| S4 | `export_kmz_inner` が 66 行・5 段 | `resolve_photo_names` / `photo_entry_bytes` / `icon_thumbnail_bytes` / `zip_entries` に分割 | kmz テスト一式 |
| S5 | `thumbnails: Vec<Option<Vec<u8>>>` の暗黙不変条件と `zip` の silent truncation。`None` が「元バイト列」を意味する二重契約 | 格納するバイト列を `Cow<'_, [u8]>` に解決して持ち（`None` の意味を消した）、`zip_entries` の先頭で長さを `debug_assert_eq!` で検査 | kmz テスト一式（`asking_for_icons_writes_one_icon_entry_for_each_photo` ほか） |
| S6 | `is_zero` はテスト以外に呼び出しが無く、`includes` は本体からのみ呼ばれるのに `pub` | `includes` を private に。`is_zero` は production 呼び出し元が無いため削除し、テストは `PhotoTimeShift::default()` との比較に変更 | `the_day_hour_minute_and_second_fields_add_up_to_one_total` |
| S7 | ゼロ上限の意味が同じファイル内で逆（`KmzIconOptions` は 0 を拒否、`KmzPhoto` は 0 を「縮小しない」で通す） | 挙動は既存公開 API なので変えず、`KmzPhoto::max_edge_pixels` の doc と共通ヘルパのコメントに「`Some(0)` は制限なし、`KmzIconOptions` の 0 はエラー」を明記 | 既存 `a_photo_with_no_limit_keeps_its_bytes` / `an_icon_limit_of_zero_pixels_is_refused` |

### Could

- 対応: 仕様 C1（`is_zero` 削除）、仕様 C4（M2 と同じ。doc を実装に合わせた）、可読性 C1（エラー文を構造体 Debug の露出から「各値と小さくする案内」に変更）、可読性 C3（容量計算のコメント）、可読性 C5（default の重複主張を 1 つに）、可読性 C7（ezTour との等価性を断定しない参照を追加）。
- 見送り:
  - 可読性 C2（`Copy` な設定値の渡し方が値/参照で不揃い）: `KmzIconOptions` を参照で渡すのは `GeotagOptions` と同じ「設定型は参照」の慣習に合わせた判断。CLI 配線前の signature 変更は差分の利得が小さい。
  - 可読性 C4（テストがエントリ順を固定）: レビュー自身が「害は小さい」としている実装固定テスト。集合比較への書き換えは別途。
  - 可読性 C6（同じ説明をモジュール doc・関数 doc・`12` で 3 回）: 未確定情報の参照先を 06 に寄せたが、説明の重複自体は残した。

### 却下した指摘

なし（Stage 1 の指摘はすべて事実と一致した）。

### 変異で確認したこと

いずれも変異を入れて対応テストが落ちることを確認し、そのあと元に戻して
`git status --porcelain` の対象ファイルが元の差分に戻ったことを確認した。

- `camera_string` を `String::from_utf8_lossy(...).trim()` に戻すと
  `camera_names_that_are_not_valid_utf8_are_unreadable_not_collapsed` と
  `leading_whitespace_is_part_of_the_camera_name` が落ちる。
- 末尾の NUL/空白の除去をやめると camera の 4 本
  （invalid UTF-8 / leading whitespace / trailing padding / inline の short names）が落ちる。
- `plan_with_time_shift` で shift の検査より先に写真を `read` すると
  `a_shift_that_overflows_seconds_is_refused_before_any_photo_is_read` が
  `Io` で落ちる（`State` を期待している）。

### ゲート

`./tools/check.sh` 6/6（`M241_VENDOR_IMAGE` を設定した状態。実機ポートは開いていない）。

### Stage 3 検証

未検証（別エージェントが記入する）。

## 2026-11-22 / PR #23 `feat-geotag-cli` の Stage 2 修正

仕様検収（Must 1 / Should 4 / Could 3）と可読性レビュー（Must 3 / Should 4 / Could 4）への対応。
変更は `crates/m241-cli` と docs のみで、`m241-core` / `m241-flash` / `m241-gui` は変更していない。
実機ポートは開いていない。Stage 1 がレビューした head は `fc70ecf`。

### 仕様検収の Must への対応

| # | 指摘 | 対応 | 固定するテスト |
|---|---|---|---|
| M1 | スコープ違いの `--photo` / `--camera-*` を警告して無視する挙動（`logcmd.rs:749-763`）にテストが無い | 無視されるフラグを 1 つずつ stderr に出し（下の可読性 S1 で表を grammar に一本化）、`--scope all` + `--photo` と `--scope selected` + camera flags の両方を、警告文と「どちらの選択が勝つか」で固定 | `a_photo_flag_under_the_all_scope_is_reported_and_ignored`, `camera_flags_under_the_selected_scope_are_reported_and_ignored` |

### 可読性 Must への対応

| # | 指摘 | 対応 | 固定するテスト |
|---|---|---|---|
| M1 | mtime 由来の撮影時刻では「次の実行に shift 不要」が偽 | rustdoc を「`DateTimeOriginal` を書けた写真では」に限定し、`--shift-*` が非 0 で mtime 由来自動写真があれば「shift は保存されない。次の実行でも同じ `--shift-*` を渡すこと」と stderr で警告。`02-cli.md` にも例外と警告を追記 | `a_shift_of_a_photo_with_only_a_file_time_is_not_written_into_it` |
| M2 | `--photo` は展開後パスとの完全一致で、外すと無警告の no-op | `--scope selected` で 1 枚も展開後パスに一致しなければ `Error::State`（exit 1）。エラー文は「展開後のパスと比較する（ディレクトリは `<DIR>/<NAME>` を寄与）」と計画パスの実例を示す。`--photo` のヘルプと `02-cli.md` も同じ規則に修正 | `a_photo_selection_that_matches_none_of_the_planned_photos_is_an_error`, `a_selected_scope_can_name_a_photo_contributed_by_a_directory` |
| M3 | shift 要約は 0 件マッチでも「shift した」と読める | 要約行を `shift: <N> s requested (<scope>)` に変え、要求値であることを明示（実績は per-plan の `capture` と `matched`/`written`）。`selected` の 0 件は M2 でエラーになり、要約も出ない。`by-camera` の 0 件を CLI 側で数えるには core の非公開照合規則（`PhotoShiftScope::includes`）を複製する必要があるため、枚数の表示は見送り、文言と docs で誤読を防ぐ形にした | `a_photo_flag_under_the_all_scope_is_reported_and_ignored`（`requested` 表記） |

### 仕様検収の Should への対応

| # | 指摘 | 対応 | 固定するテスト |
|---|---|---|---|
| S1 | mtime フォールバック写真はシフトが永続化されず docs の記述が当てはまらない | 可読性 M1 と同じ対応（docs の例外明記＋警告） | `a_shift_of_a_photo_with_only_a_file_time_is_not_written_into_it` |
| S2 | `selected` の `<DIR>/<NAME>` 寄与パスの CLI 統合テストが無い | ディレクトリ入力 + `--photo <DIR>/<NAME>` の統合テストを追加 | `a_selected_scope_can_name_a_photo_contributed_by_a_directory` |
| S3 | `--tz-offset-minutes` と shift の併用 e2e テストが無い | tz=540 + shift=3600 で「照合は tz+shift 後、書込 `DateTimeOriginal` は shift 済みローカル時刻、位置 utc は真 UTC」を固定 | `a_tz_offset_and_a_shift_compose_without_moving_the_written_position_time` |
| S4 | dry-run の人間向け出力にシフト後 capture 時刻が無い | 人間向け plan 行を `(matched UTC, gap N s, capture <ISO-8601>)` に拡張（書き込み実行も同じ行）。docs も同じ表現に更新 | `a_photo_flag_under_the_all_scope_is_reported_and_ignored` |

### 可読性 Should への対応

| # | 指摘 | 対応 | 固定するテスト |
|---|---|---|---|
| S1 | スコープ→フラグ表が grammar と handler で二重化 | `LogGeotagOptions::flags_for_other_scopes()` に一本化し、handler は返り値を 1 行ずつ warn するだけにした（handler 側の表と 15 行の重複を削除） | 上記 2 本の警告テスト |
| S2 | `--scope` の綴りが 3 箇所（ValueEnum / `label()` / `required_if_eq` リテラル）に複製 | `SCOPE_SELECTED` / `SCOPE_BY_CAMERA` 定数に集約し、`label()` と `required_if_eq` が同じ定数を使う。`label()` が clap の possible value 名と一致することをテストで固定（rename 時に parse 条件が黙って消えるのを防ぐ） | `every_scope_is_spelled_the_same_way_wherever_it_is_written` |
| S3 | 新テストが「shift して再マッチ」を固定していない | `matched_utc`（と `gap_seconds`）を assert し、全点が同座標の合成トラックでもマッチ先が shift 後であることを固定。selected / by-camera / tz 併用の 3 本を強化 | `a_selected_scope_shift_moves_only_the_named_photos_clock`, `a_by_camera_scope_shift_moves_only_photos_from_that_camera`, `a_photo_flag_under_the_all_scope_is_reported_and_ignored`, `camera_flags_under_the_selected_scope_are_reported_and_ignored` |
| S4 | `geotag_options()` が clap 既定値の写し | `Cli::try_parse_from` で実際に parse して既定値を取得するヘルパに変更（既定値の定義は `cli.rs` の 1 箇所だけになる） | 既存テスト群の前提（コンパイルと既定値依存テストで担保） |

### Could

- 対応:
  - C1（テスト用 JPEG ビルダの重複）: `synthetic_jpeg_exif(datetime, camera)` に統合し、
    `synthetic_jpeg_with_datetime` / `synthetic_jpeg_with_camera` は薄い wrapper にした。
  - C2（テキスト dry-run の capture 時刻）: 仕様検収 Should 4 と同じ変更。
  - C3（`--scope` だけ指定して shift 0 は no-op）: `--scope` のヘルプに「shift フラグが操作本体。
    all-zero shift の `--scope` は何も動かさない」と明記（警告は追加していない）。
  - C4（語と doc の細部）: `cli.rs` モジュール doc の parse 時ルール列挙を更新、`--photo` のヘルプ 1 行目を
    「`<PHOTOS>` から選ぶ」に変更、`shift()` / `shift_scope()` を `photo_time_shift()` /
    `photo_shift_scope()` に改名、`geotag_document` に shift を渡して `total_seconds()` の再計算を削除。
- 見送り:
  - C3 の警告化: `selected` の 0 件は仕様検収 M2 でエラーになり、`by-camera` の 0 件を CLI 側で
    検出するには core の非公開照合規則を複製する必要がある。ヘルプでの明記に留めた。
  - C4 の `--photo` 改名（`--select-photo` 等）: 引数名は Stage 1 仕様検収で承認済みのため変更しない。

### 却下した指摘

なし（仕様検収・可読性の Must / Should はすべて事実と一致した）。

### 変異で確認したこと

いずれも変異を入れて対応テストが落ちることを確認し、そのあと元に戻して
`git diff` が変異前と同じ内容に戻ったことを確認した。

- スコープ違いの警告文を変えると
  `a_photo_flag_under_the_all_scope_is_reported_and_ignored` が落ちる。
- mtime 由来自動写真の警告条件を偽にすると
  `a_shift_of_a_photo_with_only_a_file_time_is_not_written_into_it` が落ちる。
- `plan_with_time_shift` に `PhotoTimeShift::default()` を渡すと
  `a_selected_scope_shift_moves_only_the_named_photos_clock` と
  `a_by_camera_scope_shift_moves_only_photos_from_that_camera` が `matched_utc` / `capture_utc` で落ちる。
- `--photo` 0 件マッチのエラー判定を外すと
  `a_photo_selection_that_matches_none_of_the_planned_photos_is_an_error` が落ちる。

### ゲート

`./tools/check.sh` 6/6（`M241_VENDOR_IMAGE` を設定した状態。実機ポートは開いていない）。

### Stage 3 検証

未検証（別エージェントが記入する）。

---

## 2026-11-22 / W2a DA 全文 SHA-256（PR #24、Stage 1 → Stage 2）

対象: `feat-da-digest`（base `origin/main` = `40efd53`）。Stage 1 は仕様検収
（Must 0 / Should 3 / Could 4）と可読性（Must 2 / Should 4）の 2 本で、レビューした
head は `6945728`。この節は Stage 2 の対応記録で、Stage 3 は別エージェントが記入する。

### 対応表（Must）

| 指摘 | 対応 |
|---|---|
| 可読性 M1: `check_da_image_against` / `DaLoader::load_against` が生 `&str` を受け取り、rustdoc が pin の差し替えを正当化している。機械ゲートも無い | `DaDigest` トークンに変更。フィールドは private で、通常ビルドの構築子は `DaDigest::vendor()`（= `DA_IMAGE_SHA256`）だけ。テスト用 `DaDigest::for_test_da` は `#[cfg(any(test, feature = "simulator"))]` の下（`FakeDa` と同じ feature 慣習）。CLI の本番 5 箇所は `da_digest()` が返す `DaDigest::vendor()` だけを渡せる。`tools/check_safety.py` に検査 8「`DaDigest::for_test_da` の直前が `#[cfg(any(test, feature = "simulator"))]`」を追加し、`docs/dev/11-safety-review.md` の ✓ 表に追記。`crates/m241-flash/tests/safety_gate.rs` に `a_da_digest_constructor_outside_its_cfg_gate_is_reported` を追加（cfg を外す変異で落ちる） |
| 可読性 M2: `the_vendor_da_hashes_to_the_pinned_digest_and_loads` が env 無しで無言 skip し、`docs/dev/11-safety-review.md` が 🧪 として引用している | テスト名を `the_vendor_da_matches_the_pinned_digest_and_loads_when_the_vendor_da_is_available` に変更。skip 時に `eprintln!` で理由を出す。`docs/dev/11-safety-review.md` の該当行を「条件付き（CI では未実行）」に変更し、pin 値そのものは CI で走る `the_loader_constants_are_pinned_to_the_documents_and_the_read_crate` がリテラル固定することを明記 |

### 対応表（Should / Could）

| 指摘 | 対応 |
|---|---|
| 仕様 S1 / 可読性 S1: pin 値の CI 回帰が無い | `the_loader_constants_are_pinned_to_the_documents_and_the_read_crate` に `DA_IMAGE_SHA256` のリテラル比較と `DaDigest::vendor().sha256()` の比較を追加（CI で常に走る） |
| 仕様 S2: public な逃げ道（M1 と同じ） | `DaDigest` トークン化で解消（上記） |
| 仕様 S3: ベンダーファイル無し環境で skip する検証の記録 | `M241_VENDOR_IMAGE` を設定して `./tools/check.sh` を実行し 6/6 を確認（下記「ゲート」）。CI では条件付きテストが skip することを `docs/dev/11-safety-review.md` に明記 |
| 仕様 C1: `fw flash` 側に「正しい先頭・誤 digest」テストが無い | `flash_refuses_a_same_size_da_with_the_right_head_but_a_different_digest` を追加 |
| 仕様 C2 / 可読性 S1: `digest::matches` を再利用していない・拒否文が常に「pinned vendor digest」と言い切る | 比較を `m241_core::firmware::digest::matches` に変更。拒否文は期待値が pin 値のときだけ「the pinned DA_v3.8.4.bin digest」、それ以外は「the expected digest」と表示し、次の行動（ベンダーパッケージの `DA_v3.8.4.bin` を使う）を書く |
| 仕様 C4: `--da` フラグ説明に digest が無い | `docs/reference/02-cli.md` の `fw probe-da` フラグ行に追記 |
| 可読性 S2: docs が実際は呼ばない `check_da_image` を名指し | `AGENTS.md` §2.5、`docs/reference/02-cli.md`、`docs/dev/11-safety-review.md` を、CLI が `check_da_image_against` / `load_against` に `da_digest()` を渡し、`check_da_image` / `load` は同じ pin 済みチェックである、という記述に更新 |
| 可読性 S3: fingerprint / digest / hash / SHA-256 の語の揺れ | 概念語を `digest` に統一（`DaDigest`、`da_digest()`、テスト名を `..._wrong_digest` / `..._different_digest` / `..._matches_the_pinned_digest_...` に改名） |
| 可読性 S4: `cfg(not(test))` の本番分岐がテストで実行されない | `da_digest()` の doc に「この分岐は `cargo test` ではコンパイルされない（テストは `#[cfg(test)]` 側を通る）」と明記し、フィクスチャで通るテスト名に条件（`with_the_test_fixture_da`）を付けた |
| 可読性 C1: モジュール doc の 4+1 チェックが数えにくい | 「ジャンプ前の 4 チェック + その後 DA 自身が返す 20 バイト report が 5 番目」と明記 |
| 可読性 C2 / C4: fixture doc の主語が移動後になっていない・2 つの fixture の差が不明 | `da_fixture_bytes` の doc を「書き込む合成 DA」に直し、`m241-flash` の `da_image()` との差（`DA_REPORT_OFFSET` の fake report のみ）を明記 |
| 可読性 C3: `pub(super)` の理由が無い | `da_fixture_bytes` の doc に「親の `da_digest` が同じバイトをハッシュするため」と明記 |
| 可読性 C5: `run_against` の doc が実装詳細に読める | `load` の check を切り出したものであり、`DaDigest` トークンにより逃げ道ではない、という主語に書き直した |

### 却下・見送り

- 仕様 C3（`fw probe-da` も DA の SHA-256 を表示する）: `fw probe-da` の標準出力・
  `--json` 契約と `docs/reference/02-cli.md` の出力節を変える別変更になる。§2.5 の承認
  成果物はもともと承認者が `--da` のパスから独立に SHA-256 を再計算する運用で、
  W2a（安全ゲートの強化）の範囲外。別 follow-up 候補として残す。
- 可読性 C6（`assert_eq!(opcodes(), &[] as &[u8])` を `is_empty()` に）: 新規テストは
  `is_empty()` にしたが、既存行（旧テスト）はファイル内の既存パターンを揃えるため
  そのまま。全置換は本 PR の目的（安全ゲート）と無関係な差分を増やす。

### ゲート

`M241_VENDOR_IMAGE=".../M-241 Firmware V1.13 update package/01017-01C ... V1.13_GR-241_HLX.bin" ./tools/check.sh`
→ 6/6（fmt / clippy / test / docs / design / safety）。実機ポートは開いていない。
この設定では `M241_VENDOR_IMAGE` の隣の `DA_v3.8.4.bin` が使われ、条件付きの
実 DA 受理テストも実行される（CI ではベンダーファイルが無いため skip。skip は
`eprintln!` で出力され、pin 値のリテラル回帰は CI でも走る）。

### Stage 3 検証

未検証（別エージェントが記入する）。
