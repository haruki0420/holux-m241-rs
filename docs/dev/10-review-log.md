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

---

## 2026-11-21 / GUI のログ画面と接続（`feat/gui-eztour-live` → `fix/gui-eztour-live`）

対象: `crates/m241-gui/`（`app.rs`, `screens/connect.rs`, `screens/log.rs`, `screens/settings.rs`, `widgets/rail.rs`, `tests/render.rs`）と `docs/dev/01-design-decisions.ja.md`。
仕様書: `DESIGN.md` §3・§5・§6、`docs/dev/07-eztour-spec.ja.md` §5・§8。実機ポートは開いていない。

- **Stage 1 並行レビュー**（仕様検収＋可読性、互いの結果を見せずに並行実施）: **不合格**。
  変異 13 件中 7 件が生存（M2 `$PMTK182,5` 送信、M3 確認なしジオタグ、M4 無限再生、M6 スライダー無効化、
  M7 速度パネル常時メートル、M8 rail transition 無視、M9 rail の 4px を 0）。仕様違反は
  E5（軌跡が単色・幅 2）、E6（始点／終点マーカー無し）、E4（`fit_all` 後の追従が点を選ばない）。
- **Stage 2 修正（別エージェント）**: 6 ファイル＋設計記録 1 ファイル。

| # | 指摘 | 解消 | 確認箇所・検出テスト |
|---|---|---|---|
| 1 | `probe_client` の送信内容が未固定（M2） | 解消 | `probe_client` を crate 内に落とし、`probe_client_sends_only_read_requests` で `$PMTK605` / `$PMTK182,2,8` / `$PMTK182,2,1` の 3 通だけを assert。変異 M2 で FAIL |
| 2 | ジオタグの確認ゲートが未固定（M3、可読性 Must 1） | 解消 | `GeotagConfirmation` トークン（`confirm_geotag_write` だけが発行）を `apply_geotags` が要求。`asking_to_geotag_changes_no_bytes_until_the_write_is_confirmed` / `confirming_needs_an_open_prompt` を追加。変異 M3 で FAIL |
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
- **変異テスト**: 8 変異を実行し 7 検出。M4a（末尾で `playing = false` を消し `last_step_at` だけ更新）は生存 — 次のティックで再度 `step_next` が失敗して停止するため、状態は 1 フレーム遅れて同じ結果になる。害がない変異として記録する。M4c（`self.playing` を停止前に返す）は検出。
- **未検証**: `probe_serial_port` の実機動作、追従の実描画、GUI 経由の実 JPEG 書き込みは 05-verification.md に記録が無く、**未検証**のまま（合格に含めない）。
