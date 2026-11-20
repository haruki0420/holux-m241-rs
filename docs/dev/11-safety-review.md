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
2. **`0x00000000` を含む範囲を消せる経路がないか。** 👁
   `0xD5` は範囲が触れるセクタを、書く直前に消す（同 §5.6）。`start = 0` は
   リセットベクタを消し、装置は起動しなくなる。範囲定数が 0 を含まないことを
   ユニットテストで固定し、書き込み経路が 1 本だけであることをレビューで確認する。
   *現状:* `crates/m241-flash/src/da.rs` は `DownloadAgent::unsupported` のみの
   拒否スタブで、範囲定数もそのテストも未実装である。DA 書き込みを実装する差分で
   必ず範囲定数と `start > 0` のテストを追加し、その時点で再確認する。
3. **書き込み後に読み戻して照合しているか。** 👁
   書いたバイト列をそのまま信じない。BootROM 経由で同じ範囲を読み、イメージと
   比較する。比較は `m241-core` の読み取り API で行う（書き込みクレートが読み取り
   側を再実装しない）。
4. **失敗時に装置を既知の状態に戻せるか。** 👁
   途中で失敗したとき、どこまで書けたかを報告し、リセット（`0xD9`）後に
   BootROM へ再入する手順が案内されること。`0xD9` は応答を返さないので、
   待ってはいけない（同 §8.4）。
5. **citation のない定数がないか。** 👁
   `m241-flash` の全ての定数は `docs/reference/01-da-protocol.md` の関数と
   命令範囲を指すこと。指せないなら、その定数は書けない（`CONTRIBUTING.md`）。

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
