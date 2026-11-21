# 安全性レビュー

読み取りルーチンと書き込みルーチンをクレート境界で分ける。このページはその
分離が何を保証し、何を保証しないかを固定する。**書き込み経路と読み取り経路の
レビューは、このページの観点を全部埋めてから通す。**

`✓` は機械的に検査できる（`tools/check_safety.py` が落とす）、`👁` は人か
エージェントが差分を読んで判定する、の印。

## なぜ分けるのか

シミュレータでの検証は正しさの証明ではない（[`05-verification.md`](05-verification.md)
§3）。クライアントとシミュレータが同じ関数を共有していれば、その関数の誤りは
シミュレータでも同じように「正しく」見える。残る担保は逆解析の citation と実機
確認とレビューだけで、そのうちレビューは忙しくなると抜ける。

分離はそのレビューを減らすためのものではない。**読み取り専用のコードに書き込み
ルーチンがリンクされないことを、レビューではなくクレート依存の向きで防ぐ**ための
もの。`m241-core` にはログ消去（`$PMTK182,6`）やフラッシュ消去・書き込み
（`0xD3` / `0xD4` / `0xD5` / `0xD9`）のルーチンが無い。そのため `m241-core` だけを
リンクしたバイナリは、`LogEraser::erase` や `DownloadAgent` を誤って呼び出すことが
できない。

ただし、**コンパイルだけで「一切バイトを送れない」ことまでは保証していない。**
`m241-flash` が同じシリアル層の上に載るため、`m241-core` は低レベルの送受信・
フレーミングプリミティブ（`Port::open`、`Transport::write_all`、`pmtk::line`、
`Frame`）を公開したままにしている。これらを使って任意の生バイト列を意図的に
組み立てて送ること自体は型システムでは防げない。その穴を塞ぐのは、静的検査
（`tools/check_safety.py`）と下のレビュー観点の役目である。

そのうえで、残った危険（書き込み経路そのものの正しさ）はこのページのチェック
リストで読む。分離で消えたのは「読み取りのつもりで消去・書き込みルーチンを呼べる」
という種類の事故だけで、書き込みの判断ミスは消えていない。

## クレート構成

```
m241-core（読み取り専用）←── m241-flash（書き込み）
        ↑                          ↑
   m241-gui                   m241-cli
```

- **`m241-core`** — PMTK の読み取り（`$PMTK605` / `$PMTK182,2,8` /
  `$PMTK182,7`）、BootROM の読み出し（`0xA2` / `0xAF` / `0xA4`）、ログの解析、
  ファームのパッチ計画。**破壊的コマンドの実装（ログ消去・フラッシュ消去/書き込み
  ルーチン）を 1 つも持たない。**（`m241-flash` が共有する低レベルの `Port` /
  `Transport` / `Frame` / `pmtk::line` プリミティブのみ公開している。）
- **`m241-flash`** — `$PMTK182,6`（`crates/m241-flash/src/erase.rs`）と
  Download Agent（[`01-da-protocol.md`](../reference/01-da-protocol.md)）。
  `m241-core` に依存してよい。**逆は禁止。**
- **`m241-cli`** — 両方に依存する。`fw flash` と `log erase` があるため。
- **`m241-gui`** — `m241-core` のみ。今は書き込み経路を呼ばないので、書き込み
  クレートをリンクしない。

依存の向きが分離の土台なので、`✓` `cargo tree -p m241-core --edges normal,build,dev`
および `cargo tree -p m241-gui --edges normal,build,dev` に `m241-flash` が現れない
こと、`cargo tree -p m241-flash --edges normal,build,dev` に `m241-core` が現れる
こと、`crates/m241-core/Cargo.toml` と `crates/m241-gui/Cargo.toml` が `m241-flash`
を名指ししないことを検査する（dev / build 依存も含む。`cargo test -p m241-core` や
`cargo test -p m241-gui` からリンクできてしまえば分離の意味が無い）。

## 書き込み経路のレビュー（必須）

前提: この節は `m241-flash` の差分に対して行う。`m241-core` の差分に
`0xD3` / `0xD4` / `0xD5` / `0xD9` / `$PMTK182,6` が現れたら、それは分離の破れなので
レビューではなく設計の差し戻し。

1. **消去範囲がハードコード定数か。** 👁
   ユーザー入力・ファイル内容・装置の応答から計算した値を消去範囲にしてはならない。
   `docs/reference/01-da-protocol.md` §4.2 のとおり `0xD3` の範囲は
   `(start, end)` の生バイトで、`end` は長さではない。定数は
   `m241-flash` のソース上でリテラルとして読めること。
   *現状:* `crates/m241-flash/src/da.rs` の `FLASH_WRITE_START` / `FLASH_WRITE_LENGTH`
   が唯一の範囲で、どちらもリテラル。`0xD3` に渡す `end` は
   `FLASH_WRITE_END = start + length - 1` として定数から導出し、
   `check_layout` が両者が期待値どおりか確認する。範囲を受け取る公開関数は無い。
2. **`0x00000000` を含む範囲を消せる経路がないか。** 👁
   `0xD5` は範囲が触れるセクタを、書く直前に消す（同 §5.6）。`start = 0` は
   リセットベクタを消し、装置は起動しなくなる。範囲定数が 0 を含まないことを
   ユニットテストで固定し、書き込み経路が 1 本だけであることをレビューで確認する。
   *現状:* `crates/m241-flash/src/da.rs` に書き込み経路を実装した。範囲は
   `FLASH_WRITE_START` / `FLASH_WRITE_LENGTH` のリテラルのみで、`check_layout`
   が期待値と照合する。開始アドレスの DA 側表現が未確定
   （`docs/reference/01-da-protocol.md` §4.2 は `0x00200000`、`m241_core` の
   `BASE_ADDR_FIRMWARE_M3318` は `0` と書き、実機の読み出しで決着していない）ため、
   出荷定数は `0` であり、したがって**この経路は現在どの実機にも書き込めない**。
   `check_layout` は定数 `UNRESOLVED_STARTS` に**両方の候補**（`0` と `0x00200000`）
   が載っている間、開始がそのどちらでも `Error::Safety` を返す。定数を片方に
   取り替えるだけでは解禁されず、候補リスト自体を空にすることが、
   `docs/dev/05-verification.md` の行で決着を記録する編集になる。
   テストは非公開の `write_range` にテスト専用の開始アドレスを渡してプロトコルだけを
   検証している（🧪）。
   `crates/m241-flash/src/da.rs` の
   `the_shipped_start_is_zero_and_the_layout_check_refuses_it` と
   `the_layout_check_refuses_both_unresolved_starts` が固定する。
   開始アドレスが実機で確定した時点で、定数・候補リスト・テスト・この段落を同時に更新する。
3. **書き込み後に読み戻して照合しているか。** 👁
   書いたバイト列をそのまま信じない。BootROM 経由で同じ範囲を読み、イメージと
   比較する。比較は `m241-core` の読み取り API で行う（書き込みクレートが読み取り
   側を再実装しない）。*現状:* `PendingWrite::verify_written` が `0xD9` を送ってから
   `m241_core` の `MtkBootRom` で読み戻し、先頭の不一致オフセットを報告する。
   読み出しそのものは再実装していない（比較だけがこのクレートにある）。🧪
   読み出しアドレスは `BASE_ADDR_FIRMWARE + da_start` で、フェイクが記録した
   最終読み出しアドレスとの一致をテストで固定している。ただし
   **DA ビューと BootROM ビューの写像そのものが未検証**（`FLASH_WRITE_START` の項）。
   テストが固定するのは「クライアントが base を足す」ことであって、
   「base を足すのが実機で正しい」ことではない。
4. **失敗時に装置を既知の状態に戻せるか。** 👁
   途中で失敗したとき、どこまで書けたかを報告し、リセット（`0xD9`）後に
   BootROM へ再入する手順が案内されること。`0xD9` は応答を返さないので、
   待ってはいけない（同 §8.4）。*現状:* `0xD5` 送信後の失敗はすべて
   `PartialWrite::recovery` の文面を伴う。書けたバイト数、`0xD9` の送信有無、
   `--assume-brom` での再入、自分のバックアップの書き戻しを名指しする。🧪
   クライアントが `0xD9` の後に読まないことは
   `the_restart_does_not_read_a_reply` が、書き込み経路が `0xD9` を送らないことは
   `the_shipped_write_path_never_reaches_the_wire` が固定する。
5. **citation のない定数がないか。** 👁
   `m241-flash` の全ての定数は `docs/reference/01-da-protocol.md` の関数と
   命令範囲を指すこと。指せないなら、その定数は書けない（`CONTRIBUTING.md`）。
   *現状:* `da.rs` の全定数が同ページの節か `03-hardware-facts.md` の行を指す。
   指せない値（`REPLY_TIMEOUT`、DA 側の開始アドレス）は
   プロトコル定数ではない／未検証であると本文に明記した。2 つの最終 `0x5A` の間の
   バイト数のように、このクライアントが正当化できる値が 0 しかないものは定数にせず、
   `expect_final_acks` の doc に未検証として書いた。

### DA ロードと `fw probe-da`（今回追加）

開始アドレスを確定するための経路を足した。この経路そのものの安全上の位置づけを
ここに固定する。

1. **DA ロードは RAM への書き込みであり、フラッシュ書き込みではない。** 👁
   `DaLoader::load` が送るのは BootROM の `0xA1`（RAM に半ワードを書く）と 1 回の
   `0xA8`（`0x00100800` へジャンプ）だけ。書き込み先は
   `0x00100800`–`0x0010422F` のチップ内 SRAM で（`docs/reference/01-da-protocol.md`
   §2, §9）、電源を切れば消える。`0xD3` / `0xD4` / `0xD5` / `0xD9` は 1 度も送らない。
   ただしロード後の DA は次のコマンドでフラッシュを操作できるため、確認
   （`--yes` だけ。`fw probe-da` にタイプ入力のプロンプトは無い）は必須のままにした。
   *固定するテスト:* `the_loader_writes_the_whole_image_to_the_da_base_and_starts_it`
   が偽 DA の受けた opcode を `0xA1` / `0xAF` / `0xA8` に限り、
   `the_read_path_never_sends_a_write_opcode` が読み出し経路の opcode を `0xD6` に限る。
   ロード範囲が BootROM のスタック `0x00100000`–`0x001007FF` と重ならないことは
   `the_load_range_does_not_touch_the_bootrom_stack` が区間として固定する。🧪
2. **probe は `0xD3` / `0xD4` / `0xD5` / `0xD9` を呼ばない。** 👁
   読み出しは `DownloadAgent::read_range` の `0xD6` だけで、CLI がアドレスを決める場所は
   2 つに限られる: 読み出しの一覧は `m241_flash::da::read_candidates` の
   `PROBE_ADDRESSES`（= `UNRESOLVED_STARTS`）に固定され、`fw probe-da` はアドレスを
   持たない。CLI の判定 `base_verdict`（`crates/m241-cli/src/fw.rs`）は、その 2 つ以外の一覧を受け取ると
   `Error::State` で拒否する（`the_probe_verdict_refuses_anything_but_the_two_candidate_slots`）。
   `read_range(address, length)` 自体は `pub` の読み出し API で任意の `u32` を受けるが、
   範囲を記録・消去・書き込みはしない（範囲を受け取る書き込み関数は private の
   `write_range` だけ）。`0xD3` に相当するコマンドも送らないので、後続の `0xD5` が
   書き込みに使える範囲は更新されない（`the_read_path_never_sends_a_write_opcode` が
   偽 DA の `recorded_range() == (0, 0)` を確認）。🧪
   この不変条件は **`fw probe-da` の本体についても**、
   `probe_da_reads_both_candidates_through_the_fake_and_sends_no_write_opcode` が
   `m241-cli` の偽 DA（`crates/m241-cli/src/testing.rs` の `fake_da`）で `probe_da` を最後まで
   駆動して固定する: 受けた opcode が `0xA1` / `0xAF` / `0xA8` / `0xD6`（と `0xD6` の
   `0x5A` / `0xA5`）だけであること、`0xD3` / `0xD4` / `0xD5` / `0xD9` が無いこと、`0xA1` の
   アドレスが DA の RAM 範囲に収まること、`0xA8` が `0x00100800` に count `0` で 1 回だけ
   であること、リセット線を脈打たないこと。🧪
3. **開始アドレスが確定するまで `FLASH_WRITE_START` は拒否のまま。** 👁
   `fw probe-da` は判定（`BaseVerdict`）を表示するだけで、`da.rs` の定数・
   `UNRESOLVED_STARTS`・`check_layout` を一切書き換えない。確定は次の 2 段階で行う:
   `docs/dev/05-verification.md` §1 に実機の読み出し結果を記録し、その行を根拠に
   `UNRESOLVED_STARTS` を空にし `FLASH_WRITE_START` を確定値にする。probe が
   `Settled(0x0020_0000)` を返しても、それだけで書き込みは開かない。

### この経路で担保していないこと（マージ時に「担保済み」と読まない）

以下はコードにあるが、**実機でもフェイクでもテストで固定していない**か、
固定したのは今回で初めてである。前者は未検証として列挙する。

| 項目 | 状態 | 固定するテスト |
|---|---|---|
| `0xD5` の最初セクタ消去失敗 → `0xA5` | 命令列のみ。実機で失敗を起こしていない。フェイクが模す | `a_failed_first_erase_is_a_rejected_error_with_nothing_written`（今回追加） |
| `0xD9` が応答を返さないこと | 命令列のみ（`0x00101920`）。実機で測っていない | `the_restart_does_not_read_a_reply`（今回追加） |
| 短い最終パケットのゼロ詰め | 書き込み方向は未検証。§4.4 は読み出し側の直列化から読んだ記述 | `the_final_packet_is_the_short_one_and_is_zero_filled`（今回追加、フェイクが受けた payload を見る） |
| 拒否された `0xD3` が 1 バイト `A5` | 命令列のみ。実機で拒否させていない | `a_refused_range_is_one_byte_and_a_rejected_error` |
| `0xD5` の 2 つの最終 `0x5A` の間にバイトを送るか | 未確定。送らない形だけ実機で通っている | `a_full_write_consumes_exactly_its_replies` |
| DA ビュー ↔ BootROM ビューの写像 | 未検証。開始アドレスの決着待ち | `the_read_back_address_is_the_bootrom_view_of_the_da_start`（クライアントの式だけ） |
| 最終バックアップ照合（`0xD5` 後の DA 側通しチェック） | §4.4 の 2 番目のチェック。フェイクは模さない | なし |
| 実機での書き込み経路 | 一度も動いていない。`05-verification.md` §1 の手動フラッシュは本コードではない | なし |


## 読み取り経路のレビュー（必須）

前提: この節は `m241-core` の差分に対して行う。`✓` の項目は
`tools/check_safety.py` が落とすので、レビューでは「なぜその検査で足りるか」と
静的スキャンで拾えない迂回が無いかを確認する。

1. **制御線を触る箇所が、NMEA 無音確認の後だけか。** ✓ + 👁
   `set_control_lines` を本番コードから呼んでよいのは BootROM 入口
   （`crates/m241-core/src/protocol/brom.rs`）だけ。しかも呼ぶ前に NMEA の
   トライステートが `Silent` を返していること。`Streaming` でも `Unknown` でも
   触らない（`AGENTS.md` §2.1）。`✓` は「`crates/m241-core/src/protocol/brom.rs` /
   `crates/m241-core/src/protocol/port.rs` / `crates/m241-core/src/protocol/simulator.rs`
   の正確なパス以外に `set_control_lines` が現れない」こと（basename 一致は
   使わない）。`👁` は「本番呼び出し箇所が必ず `StreamState::Silent` ゲートの
   後ろにある」こと。*現状:* `m241-core` 内の本番コードに `set_control_lines` の
   呼び出し箇所は 0 件（リセット線経由の BootROM 突入は未実装。トレイト定義・実装と
   シミュレータ・テストのみ）。将来 `brom.rs` へ実装を足す際にこの観点で必ず確認する。
2. **破壊的コマンドが読み取り API から到達できないか。** ✓ + 👁
   `$PMTK182,6`、`0xD3`、`0xD4`、`0xD5`、`0xD9`（および `b'\xd3'` や `"\xD5"` の
   ようなバイトリテラル・16 進エスケープ表記）が `crates/m241-core/src` および
   `crates/m241-core/tests` のコードに無いこと。`//` コメント（コメント専用行および
   行末コメント。文字列リテラル内の `//` は除外しない）は対象外。
   `erase` / `erase_*` / `flash_write` / `program` / `program_*` という識別子も
   無いこと。例外は `erased` — 空ブロックを読むときに使う語で、書き込みではない。
   静的検査で拾えない表現（算術組み立てや `format!` による動的組み立て、生
   `Transport::write_all` への直接書き込み）が無いことは `👁` で確認する。
3. **ポートを開いたまま落ちたとき、装置が recording 停止のままにならないか。** 👁
   `log_stop` の後は必ず `log_resume` が走ること。`download_log` は失敗経路でも
   resume する（`pmtk.rs` のテスト `logging_is_resumed_even_when_the_transfer_fails`
   が固定している）。プロセスが kill された場合は防げないので、その旨を
   エラーの hint に書く。
4. **タイムアウト時に無限リトライしないか。** 👁
   `read_line` / `read_exact` は締切を持ち、締切で `Error::Timeout` を返す。
   呼び出し側がリトライする場合も回数か時間の上限を持つこと。`Error::is_fatal`
   が `false` を返すのはタイムアウトだけで、それ以外は再試行しない。

## 機械的な担保

`tools/check_safety.py` が `./tools/check.sh` の `safety` ゲートとして毎回走る。

| 検査 | 何を守るか |
|---|---|
| `$PMTK182,6` / `0xD3` / `0xD4` / `0xD5` / `0xD9`（`b'\xd3'`・`\xD5` 等を含む）が `crates/m241-core/src`・`crates/m241-core/tests` のコードに無い | 破壊的コマンドが読み取り側に戻らない |
| `erase` / `erase_*` / `flash_write` / `program` / `program_*` という識別子が無い | 書き込みの語彙が読み取り側に戻らない（`erased` は許可） |
| `cargo tree -p m241-core --edges normal,build,dev` に `m241-flash` が無い | 依存の向きが逆転していない |
| `cargo tree -p m241-gui --edges normal,build,dev` に `m241-flash` が無い | GUI が意図せず書き込みクレートをリンクしていない |
| `cargo tree -p m241-flash --edges normal,build,dev` に `m241-core` がある | 正しい向きの依存が消えていない |
| `crates/m241-core/Cargo.toml` と `crates/m241-gui/Cargo.toml` が `m241-flash` を名指ししない | 解決結果に出ない形の依存（dev 依存など）も塞ぐ |
| `set_control_lines` が `crates/m241-core/src/protocol/brom.rs` / `crates/m241-core/src/protocol/port.rs` / `crates/m241-core/src/protocol/simulator.rs` 以外に無い | DTR/RTS を触る場所が増えていない（同名別パスのすり抜けも防ぐ） |

すべての検査と `tools/check.sh` への配線は
`crates/m241-flash/tests/safety_gate.rs` で自動検証している。違反を植えた一時
ワークスペースで各ルールが落ちることと、許可された除外（`//` コメント専用行および
行末コメント、`erased`、`0xd5a7_9147` のような SHA-256 定数、許可パス内の
`set_control_lines`）が通ることを毎回 `cargo test` で確かめる。

**このゲートが守らないもの（限界）:**

- **静的字句スキャンの限界:** `0xD0 + 5` のような算術表現、`format!("PMTK182,{}", 6)` の
  ような実行時文字列組み立て、間接参照によるバイト構築、あるいは公開されている
  `Port::open` / `Transport::write_all` / `pmtk::line` / `Frame` を使った意図的な生
  フレーム送信は、正規表現ベースの静的検査では完全に防げない。
- **書き込み手順そのものの正しさ:** 範囲定数の妥当性、`start > 0` の保証、読み戻し
  照合の有無、失敗時のリカバリ案内は `tools/check_safety.py` の対象外であり、上の
  2 つのレビュー節とテストで担保する。
