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

## 2026-11-21 / 読み取り・書き込みの分離（`feat/safety-split`）

対象: `feat/safety-split`（HEAD `323baa0`、3-dot の merge-base `5c34a07`、29 ファイル
+1858/−649）。**未マージ。** 検証は修正に参加していない担当が行い、読んだのは差分・
コード・テストだけ（コミットメッセージの本文と修正者の説明は読んでいない）。
作業ブランチ `verify/safety-split` は当初 main（`5c34a07`）を指していて対象コードが
無かったため、`feat/safety-split` へ fast-forward してから測った。main は検証中に
`a882639` まで進んだが merge-base は変わっていない。

### A. 分離の成立（実測）

| 確認 | 結果 |
|---|---|
| `cargo tree -p m241-core --edges normal,build,dev` | `m241-flash` **無し**（exit 0、内容は m241-core / serialport / thiserror 系のみ） |
| `cargo tree -p m241-flash --edges normal,build,dev` | `m241-core` **有り**（先頭が m241-flash、直下に m241-core） |
| `crates/m241-core/Cargo.toml` に `m241-flash` の記述 | **無し**（grep exit 1） |
| `m241-core` ソースの破壊的コード | `PMTK182,6` はコメントのみ（`protocol/pmtk.rs:34`、`protocol/simulator.rs:376`）。`0xD3`/`0xD5`/`0xD9` はコードに **0 件**（`firmware/digest.rs:49` の SHA-256 語 `0xd5a7_9147` は除外対象で正しく通過）。`erase`/`program` 識別子はコメントと `erased`（`log/parse.rs:80`、空ブロック検出）のみ |

`set_control_lines` は `brom.rs:386`（`#[cfg(test)]` 内のテスト用スタブ。モジュール宣言は
`brom.rs:338`）、`port.rs:44/118/159/170`（trait・実装・テスト）、
`simulator.rs:584/801`（フェイクとテスト）にしか現れず、**m241-core に本番呼び出しは
1 箇所も無い**。言い換えると「NMEA 無音確認の後だけ呼ぶ」は現状 *呼ばない* ことで満たされて
おり、リセット線経路はまだ実装されていない（`PmtkClient::enter_bootrom` は
`$PMTK180` 経路のみ、`pmtk.rs:447`）。

### B. `check_safety.py` の検出力（変異テスト）

すべて `git checkout -- <file>` で戻し、都度 `git status --short` が空であることを確認。

| 植えた違反 | ゲート | 期待どおり |
|---|---|---|
| `m241-core` に `PMTK182,6` を含む文字列 | FAIL（変異後の `mod.rs:31` を検出） | はい |
| `m241-core` に `fn erase_flash() {}` | FAIL（変異後の `mod.rs:32` を検出） | はい |
| `m241-core` に `const OPCODE: u8 = 0xD5;` | FAIL（変異後の `mod.rs:32` を検出） | はい |
| `m241-core` に `0xD3` と `0xD9` | FAIL（両方検出） | はい |
| `m241-core/Cargo.toml` に `m241-flash` | FAIL（マニフェスト名指し＋依存サイクルで cargo tree も失敗＝閉じる方向） | はい |
| `frame.rs` に `set_control_lines` 呼び出し | FAIL（`frame.rs:54` を検出） | はい |
| `check.sh` から safety 行を削除 | **PASS（5/5 緑）** | **いいえ — 配線テストが無い** |
| `m241-core` の `tests/` に `PMTK182,6` と `0xD5` | **PASS** | **いいえ — 走査対象は `src/` のみ（`check_safety.py:57-58` の `CORE_SRC` と `:95-98` の `rust_sources`）** |
| `m241-core` に `const OP: u8 = 0xD4;`（AGENTS.md:96 が破壊的と明記） | **PASS** | **いいえ — 0xD4 が対象外** |
| 10 進 `const DA_ERASE: u8 = 213;`（=0xD5） | **PASS** | **いいえ** |
| バイトリテラル `const W: u8 = b'\xd5';` | **PASS** | **いいえ** |
| `const STEP: u8 = 0xD4 + 1;`（算術で 0xD5） | **PASS** | **いいえ** |
| `format!("PMTK182,{}", 6)` で文を組み立て | **PASS** | **いいえ** |
| `log/port.rs`（basename だけ許可リストと一致）に `set_control_lines` | **PASS** | **いいえ — 判定は `path.name`（`check_safety.py:152`）** |
| `brom.rs` 内に NMEA 確認なしの `set_control_lines` 呼び出しを追加 | **PASS** | 仕様どおり（👁 項目。ただし「brom.rs ならどこでも通る」） |
| `m241-gui` に `m241-flash` 依存を追加 | **PASS** | **いいえ — 検査は core の木だけ（`check_safety.py:210-227`）** |

過剰検出の確認（正常側は落ちないこと）:

| 正常なコード | 結果 |
|---|---|
| `m241-flash/src/da.rs` に `0xD4` 定数追加（書き込み側） | PASS（正しい） |
| `m241-core` のコメント行に `$PMTK182,6` / `0xD5` | PASS（正しい。`code_lines` が行頭 `//` を除外） |
| `0xd5a7_9147`（SHA-256 の語） | PASS（正しい。境界の先読みが効いている） |
| コード行の末尾コメントに単語 `erase`（`pub const P: u8 = 0; // ... the erase ...`） | **FAIL（過剰検出）** — 行頭 `//` しか除外しないため。`11-safety-review.md:90` の「コメント行は対象外」より狭い |

### C. 移設の完全性

- **`da.rs`** — 旧 `m241-core/src/protocol/da.rs` と同じ「拒否するだけのスタブ」で、
  `unsupported()` が `Error::State` を返す点も同じ（`m241-flash/src/da.rs:46-64`）。
  移設時に `0xD3`/`0xD5`/`0xD9` の定数と、その値を固定するテスト 2 件が加わった
  （`:30,37,44`、`:71-79` と `:81-88`）。実装は増えていない（据え置き＋定数）。
- **`erase.rs`** — `$PMTK182,6` を XOR チェックサム付きで送る（テストが
  `b"$PMTK182,6*23\r\n"` を固定、`:289-298`）、ACK をタイムアウト付きで待つ
  （`:156-172`）、`Rejected`/`Timeout`/`Framing` を返す。**ただし main の `erase_log`
  から 2 点後退している**:
  1. main は応答プレフィックスに `$PMTK001,182,6` を要求していた（`git show
     main:crates/m241-core/src/protocol/pmtk.rs`、`erase_log` 内）。移設後は
     `$PMTK001,` で止めて `command == 182` しか見ないため（`:137-147`）、
     **`$PMTK001,182,5,3`（logging stop の ACK）でも消去成功として `Ok(())` を返す**。
     プローブ用テストを一時追加して実行し、実際に受理されることを確認した（その後削除）。
  2. `BackupProof::of_bytes` の doc は「空のダンプは受理する」（`:63-64`）と書くが、
     `erase` は `saved_bytes == 0` を `Error::Safety` で拒否する（`:124-131`）。
     テスト `an_empty_backup_is_refused_before_anything_is_sent`（`:324-336`）は拒否側を
     固定しており、**doc と実装が矛盾**している。
- **`m241-flash/src/simulator.rs`** — main の `fix/simulator-d3` の知識を引き継いでいる:
  `0xD3` は記録のみでフラッシュ不変（`:570-587`）、消去は `0xD5` 側でセクタ単位
  （`:590-620`、`erase_range` は `:401-412`）、拒否は `NAK` 1 バイト（`:621-646`）、
  拒否時は以前のレンジを保持（`:649-700`）、`0xD9` は応答なし（`:734-744`）、
  `FLASH_SECTOR_SIZE = 0x4000`（`:86`）。main にあった DA 系テスト 8 件は削除ではなく
  移設され、新規 2 件（`a_jump_frame_is_echoed_part_way_and_changes_no_state`、
  `the_restart_command_sends_nothing`）が加わった。**テストの削除は 0 件**
  （`#[test]` の実数は main 256 → feat 268、`cargo test --workspace` の実測も 268 件）。
  なお `$PMTK182,6` のフェイクは `m241-flash` に存在せず（`grep 182` はコメントのみ）、
  `m241-core` のフェイクは同コマンドに応答しない（`simulator.rs:376-380`）ため、
  消去クライアントの端末間テストは無い（`Stub` のみ）。

### D. ゲート

`./tools/check.sh` = **6/6 PASS**（fmt / clippy / test / docs / design / safety、
HEAD `323baa0`。所要は 28 秒〜77 秒で、キャッシュ状態により変動）。`cargo test --workspace` = **268 件全緑**（実測。内訳: cli 7 /
core 157+6 / flash 19 / gui 48+6+6+19）。

### E. 新たに入った問題

1. **「コンパイルで保証する」という中心の主張が実測と合わない。**
   `docs/dev/11-safety-review.md:19` は「`m241-core` をリンクしたバイナリは
   `$PMTK182,6` も `0xD5` も持たない。持てない。呼ぼうにもクレートに無い」と書き、
   `AGENTS.md:29-30` も「compile-time fact」、`crates/m241-core/src/protocol/mod.rs:13-15`
   は「cannot write to a device」と書く。しかし `m241-core` は
   `Frame { pub opcode, address, count }.to_bytes()`（`brom.rs:151-155,193-198`）、
   `MtkBootRom::new` / `transport_mut`（`brom.rs:212,220`）、`Transport::write_all`
   （`port.rs:25`）、`Port::open`（`port.rs:62`）、`pmtk::line`（`pmtk.rs:95`）を公開して
   おり、**core だけをリンクしたバイナリが `$PMTK182,6*23\r\n` と `0xD3/0xD5/0xD9`
   フレームを組み立てて送れる**。実際に一時的な統合テスト 2 本を追加・コンパイル・実行して
   確認した（確認後に削除、`git status` は clean）。正しい主張は「書き込み *ルーチン* を
   呼べない（クレートに無い）」であり、「持てない／書けない」は言い過ぎ。
2. **safety ゲート自体が消えても誰も気づかない。** `tools/check.sh:13` の行を削除すると
   `./tools/check.sh` は **5/5 で緑**（実測）。design ゲートには配線を守るテストがある
   （`crates/m241-gui/tests/design_gate.rs:200-208`）のに、safety には同等のテストが無い。
   安全ゲートを review の代替に置く以上、配線は機械で守る必要がある。
3. **ゲートの穴（B 表の×）**: `0xD4`（`AGENTS.md:96` が「destructive on its own」と明記）
   が対象外、10 進・バイトリテラル・算術での別表記、`m241-core/tests/` 配下、
   basename 一致による許可リストの迂回、`m241-gui` 側の依存追加。いずれも
   「分離を戻す」変更を検出できない方向。
4. **`11-safety-review.md` のチェックリストが実装と対応していない箇所がある。**
   `:60-63` は「範囲定数が 0 を含まないことをテストで固定する（`✓`）」と `✓`（＝
   `check_safety.py` が落とす）を付けるが、`m241-flash` に消去レンジ定数も下限テストも
   存在しない（DA は拒否スタブ）。`:109-114` の表も「`0xD4` を検査する」とは書いておらず、
   実装と一致しているのは表の側。`:83-87` は `brom.rs` の「1 箇所」がゲートの後ろに
   あることを 👁 で確認すると言うが、その 1 箇所（本番呼び出し）は存在しない。
5. **GUI の拒否が実体を失った。** `attempt_flash` は `DownloadAgent::unsupported()` の
   エラー文字列ではなくハードコード文字列を返す（`crates/m241-gui/src/screens/firmware.rs:319-330`）。
   docstring（`:315-318`）は「named guard with its own reason」と言い続けているが、実際は
   `String::from(...)` である。テストで守られていた挙動ではないが、記述と実装がずれた。
6. **マージ事故らしき重複行。** `crates/m241-cli/src/main.rs:180` が
   `/// The error for a command this build parses but cannot carry out yet.` を 2 回連結
   している（main には無い）。rustdoc の 1 行目が壊れており、意図した変更ではない。
7. **`m241-core/src/lib.rs:5-6` が古い。** 「protocol — … the Download Agent's flash
   commands」と書いたままで、今回の変更で事実と合わなくなった（ファイル自体は未変更）。
8. テスト削除なし（`#[test]` は main 256 → feat 268、関数名の差分でも欠落 0。
   diff 上の `-#[test]` 7 行はすべて移設先で復活している）。`m241-core` の公開 API 縮小（`DownloadAgent`、
   `PmtkClient::erase_log`）による cli/gui の破損は無い（6/6、`cargo metadata` で
   core→[] / flash→[core] / cli→[core, flash] / gui→[core]）。
9. ドキュメントのゲート数更新は一貫（`README.md`、`AGENTS.md:14`、
   `docs/dev/08-parallel-work.md:33`、`docs/dev/09-review-protocol.md:37`）。
   `05-verification.md` §3 の「シミュレータは正しさの証明ではない」追記は妥当で、
   循環参照の説明も実装（`swap_pairs` の共有）と一致している。`check_docs.py` は
   「all links resolve」。

### 判定

**不合格**（`feat/safety-split`）。分離そのもの（破壊的コード 0、依存の向き、ゲートの
主要 6 検査、268 テスト、6/6）は成立しているが、(1) 中心文書の「コンパイルで保証」
「持てない」が core の公開 API で反証され、(2) safety ゲートの配線がテストで守られて
おらず行を消しても 5/5 で緑、(3) 消去 ACK の受理条件が main より広がって
`$PMTK001,182,5,3` を消去成功として扱う、の 3 点が「新たな問題を入れていない」を満たさない。
加えて `0xD4`・別表記・`tests/` 配下・basename 依存というゲートの穴と、
`erase.rs:63-64` と `:124-131` の doc/実装矛盾を記録する。修正は (1) 主張を
「ルーチンを持たない」に正す（`11-safety-review.md:19`、`AGENTS.md:29-30`、
`protocol/mod.rs:13-15`）、(2) `check.sh` の safety 行を守るテストを `design_gate.rs:200` と
同じ形で追加、(3) ACK のプレフィックスを `$PMTK001,182,6` に戻すか、`log_stop` の ACK を
誤受理しない条件を足す、(4) ゲートに `0xD4` と走査範囲（`tests/`）を追加、の 4 点。
