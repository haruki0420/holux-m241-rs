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
   m241-gui ───────────────────────┘
        └── m241-cli ──────────────┘
```

（`m241-cli` は `m241-core` と `m241-flash` の両方に依存する。）

- **`m241-core`** — PMTK の読み取り（`$PMTK605` / `$PMTK182,2,8` /
  `$PMTK182,7`）、BootROM の読み出し（`0xA2` / `0xAF` / `0xA4`）、ログの解析、
  ファームのパッチ計画。**破壊的コマンドの実装（ログ消去・フラッシュ消去/書き込み
  ルーチン）を 1 つも持たない。**（`m241-flash` が共有する低レベルの `Port` /
  `Transport` / `Frame` / `pmtk::line` プリミティブのみ公開している。）
- **`m241-flash`** — `$PMTK182,6`（`crates/m241-flash/src/erase.rs`）と
  Download Agent（[`01-da-protocol.md`](../reference/01-da-protocol.md)）。
  `m241-core` に依存してよい。**逆は禁止。**
- **`m241-cli`** — 両方に依存する。`fw flash` と `log erase` があるため。
- **`m241-gui`** — `m241-core` と `m241-flash` の両方に依存する。ユーザーの明示的な
  指示（2026-11-23）で、GUI を主要な操作面にし、書き込み・復旧・`probe-da`・
  `da-restart` を GUI から実行できるようにした。GUI は `m241_flash::flow` の
  セッション（`DownloadAgent` の同じ安全経路）だけを通り、破壊的オペコードを
  自分では持たない。

依存の向きが分離の土台なので、`✓` `cargo tree -p m241-core --edges normal,build,dev`
に `m241-flash` が現れないこと、`m241-core/Cargo.toml` が `m241-flash` を名指し
しないこと、`cargo tree -p m241-flash --edges normal,build,dev` に `m241-core` が
現れること、`cargo tree -p m241-gui --edges normal,build,dev` に `m241-core` と
`m241-flash` の両方が現れることを検査する（dev / build 依存も含む）。
GUI 側の新しい不変条件は次のとおり。

- GUI の書き込みは `m241_flash::flow::flash` を通る（`crates/m241-gui/src/device.rs`）。
  GUI が独自に `0xD3` / `0xD5` を送る第 2 の実装を持たない。
- `Confirmation` はファーム画面の「`WRITE` と入力」からしか作らない。`--yes` 相当の
  省略経路は GUI に存在しない。`probe-da` も `PROBE`、`0xD9` 再起動も `RESTART` の
  入力なしには実行できない。
- GUI のコードに破壊的オペコード（`OPCODE_*`、`\xNN` 形式、`$PMTK182,6`）が
  現れない。UI 文言中の `0xD9` などはコメント・文字列をマスクして除外する。

## GUI からの書き込み（2026-11-23 の設計変更）

以前は「`m241-gui` は `m241-flash` をリンクしない」ことが不変条件だった。ユーザーが
GUI を主要な操作面にするよう明示的に指示したため、この向きだけを変更した。

- 維持するもの: `m241-core` は読み取り専用（書き込み API を持たない）。範囲は
  `FLASH_WRITE_START` / `FLASH_WRITE_LENGTH` の固定値、`check_layout` /
  `check_image` / `verify_device` / 読み戻しは CLI と同じものを通す。
- 新しく固定するもの: GUI の書き込みは `m241-flash` の共有セッション
  （`m241_flash::flow::flash`）を通る。`Confirmation` はファーム画面でユーザーが
  `WRITE` を入力したときだけ構築され、`--yes` に相当するボタンや自動化経路は無い。
  `fw probe-da` は `PROBE`、`fw da-restart` は `RESTART` の入力で確認する。
- GUI は破壊的オペコードを自分では持たない（`tools/check_safety.py` がコード中の
  `OPCODE_*` / `\xNN` / `$PMTK182,6` を検査）。UI 文言・コメントは対象外。
- 実機での GUI 書き込みは未検証である。シミュレータで「ダンプ→パッチ→書き込み→
  読み戻し一致」「不一致→復旧」「WRITE 未入力では書かない」を固定している。

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
   が期待値と照合する。DA 側の開始アドレスは 2026-11-21 の実機読み出しで
   **`0x00000000` に確定**した（`docs/dev/05-verification.md` §1.2: DA アドレス
   `0` がベクタ `18 F0 9F E5` とベンダー V1.13 イメージの先頭 64 バイトを返し、
   DA アドレス `0x00200000` は未実装バスと整合する `02 02 20 42` を返した）。
   `FLASH_WRITE_START = 0`、`UNRESOLVED_STARTS` は空である。`check_layout` は
   「候補リストが空」「期待開始が確定値 `0`」「開始が期待値と一致」
   「長さが `FLASH_WRITE_LENGTH`」のすべてを満たすときだけ通し、それ以外の
   開始は拒否する。範囲はリセットベクタを含むが、それはイメージ自身の先頭
   バイトであり、`check_image` がバックアップとの差分をパッチ表の外に許さず、
   CLI は書き込み前に `verify_device` で範囲全体の一致を要求する設計だが、
   ライブラリの `write` 単体は `verify_device` を呼ばない。テストは非公開の
   `write_range` にテスト専用の開始アドレスを渡してプロトコルだけを検証する（🧪）。
   `crates/m241-flash/src/da.rs` の
   `the_shipped_start_is_zero_and_the_layout_check_accepts_it`、
   `the_layout_check_refuses_an_unsettled_start`、
   `a_pinned_range_that_disagrees_is_refused_on_its_own` が固定する。
   **このコードによる実機への書き込みはまだ一度も行っていない**ので、
   書き込み経路そのものは未検証のままである。
3. **書き込み後に読み戻して照合しているか。** 👁
   書いたバイト列をそのまま信じない。BootROM 経由で同じ範囲を読み、イメージと
   比較する。比較は `m241-core` の読み取り API で行う（書き込みクレートが読み取り
   側を再実装しない）。*現状:* `PendingWrite::verify_written` が `0xD9` を送り、
   呼び出し側の再入ステップ（CLI は NMEA probe → `$PMTK180*3B` → 115200）が
   transport を返したあと、`m241_core` の `MtkBootRom` で読み戻して先頭の不一致
   オフセットを報告する。再入が失敗した場合は「読み戻し未実施」として返し、
   不一致（実行したが違う）と区別する。読み出しそのものは再実装していない
   （比較だけがこのクレートにある）。🧪
   読み出しアドレスは `BASE_ADDR_FIRMWARE + da_start` で、フェイクが記録した
   最終読み出しアドレスとの一致をテストで固定している。DA ビューと BootROM
   ビューの写像は 2026-11-21 に先頭 64 バイトで確認された（DA `0` と BootROM
   `0x00200000` が同じイメージの先頭を返す。`docs/dev/05-verification.md`
   §1.2）。テストが固定するのは「クライアントが base を足す」ことと
   「`0xD9` 後に NMEA → `$PMTK180` で再入する」こと、そして「読み出しが
   `DUMP_CHUNK` ごとの複数 `0xA2` に分かれる」ことである。406,620 バイトを
   1 フレームで読むと 115,200 8N1 で約 35 秒かかり、`MtkBootRom` の 5 秒
   deadline を超える。`the_device_check_reads_the_span_in_chunked_bootrom_frames`
   と `the_read_back_reads_the_span_in_chunked_bootrom_frames` がフレーム数と
   各 count の上限を fake の記録で固定する。`--allow-vendor` の読み戻しも
   書き込んだベンダーイメージとの比較である。実機の全範囲読み戻しは未検証である。
4. **失敗時に装置を既知の状態に戻せるか。** 👁
   途中で失敗したとき、どこまで書けたかを報告し、リセット（`0xD9`）後に
   BootROM へ再入する手順が案内されること。`0xD9` は応答を返さないので、
   待ってはいけない（同 §8.4）。*現状:* `0xD5` 送信後の失敗はすべて
   `PartialWrite::recovery` の文面を伴う。書けたバイト数、`0xD9` の送信有無、
   `--assume-brom` での再入、自分のバックアップの書き戻しを名指しする。🧪
   部分書き込み後の書き戻しは `fw flash --recover --yes` が唯一の経路で、
   イメージがバックアップと完全一致するときだけ許可され、`verify_device` の
   代わりにベクタ / family のマーカー検査だけを行う。通常経路の
   `verify_device` は必須のままである。
   クライアントが `0xD9` の後に DA 応答を読まないことは
   `the_restart_does_not_read_a_da_reply` が、書き込み経路が `0xD9` を送らないことは
   `a_write_stops_before_the_restart_so_the_caller_can_decide` が固定する。
5. **citation のない定数がないか。** 👁
   `m241-flash` の全ての定数は `docs/reference/01-da-protocol.md` の関数と
   命令範囲を指すこと。指せないなら、その定数は書けない（`CONTRIBUTING.md`）。
   *現状:* `da.rs` の全定数が同ページの節か `03-hardware-facts.md` の行を指す。
   DA 側の開始アドレスは 2026-11-21 の実機読み出し
   （`05-verification.md` §1.2）によって `03-hardware-facts.md` の行を持つ。
   指せない値（`REPLY_TIMEOUT`）は
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
   ただしロード後の DA は次のコマンドでフラッシュを操作できるため、`--yes` による
   確認と独立レビュー（`AGENTS.md` §2.5、`--da` の完全な SHA-256 照合を含む）は
   必須のままにした。`fw probe-da` にタイプ入力のプロンプトは無く、`--yes` が唯一の
   入口である。DA ゲート（`check_da_image` / `DaLoader::load` と、CLI が呼ぶ
   同じ本体の `check_da_image_against` / `load_against`）はサイズ（14,896 バイト）・
   先頭 4 バイト・**全文 SHA-256**（`DA_IMAGE_SHA256`、`AGENTS.md` §2.5 のベンダー値）の
   3 つを照合し、長さと先頭だけ一致するファイルはポートを開く前に拒否する（W2a で実装）。
   期待ダイジェストは生の `&str` ではなく `DaDigest` トークンで渡す: 通常ビルドで
   構築できるのは `DaDigest::vendor()`（= `DA_IMAGE_SHA256`）だけで、テスト用の
   `DaDigest::for_test_da` は `#[cfg(any(test, feature = "simulator"))]` の下にある。
   CLI の本番経路は `da_digest()` が返す `DaDigest::vendor()` しか渡せない。
   *固定するテスト:* `the_loader_writes_the_whole_image_to_the_da_base_and_starts_it`
   が偽 DA の受けた opcode を `0xA1` / `0xAF` / `0xA8` に限り、
   `the_read_path_never_sends_a_write_opcode` が読み出し経路の opcode を `0xD6` に限る。
   長さと先頭 4 バイトだけが正しいファイルの拒否は
   `a_same_size_image_with_the_right_head_but_the_wrong_digest_is_refused` が固定する
   （CI で常に走る）。pin 値そのものは
   `the_loader_constants_are_pinned_to_the_documents_and_the_read_crate` が
   リテラルで固定する（CI で常に走る）。実 DA のダイジェスト一致は、ベンダー
   `DA_v3.8.4.bin` がある環境でのみ
   `the_vendor_da_matches_the_pinned_digest_and_loads_when_the_vendor_da_is_available`
   が確認する（無い環境では `eprintln!` で skip を明示する条件付きテスト。CI では
   この性質は未実行）。🧪（条件付き）
   ロード範囲が BootROM のスタック `0x00100000`–`0x001007FF` と重ならないことは
   `the_load_range_does_not_touch_the_bootrom_stack` が区間として固定する。🧪
2. **probe は `0xD3` / `0xD4` / `0xD5` / `0xD9` を呼ばない。** 👁
   読み出しは `DownloadAgent::read_range` の `0xD6` だけで、CLI がアドレスを決める場所は
   2 つに限られる: 読み出しの一覧は `m241_flash::da::read_candidates` の
   `PROBE_ADDRESSES`（独立定数 `0` と `0x00200000`）に固定され、`fw probe-da` は
   アドレスを持たない。CLI の判定 `base_verdict`（`crates/m241-cli/src/fw.rs`）は、その 2 つ以外の一覧を受け取ると
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
3. **開始アドレスは確定し、ゲートは開いた。** 👁
   2026-11-21 の実機読み出し（`docs/dev/05-verification.md` §1.2）を根拠に、
   `UNRESOLVED_STARTS` を空にし、`FLASH_WRITE_START = 0` を確定値とした。
   `check_layout` は候補リストが空で、期待開始が確定値 `0` と一致し、開始が
   その期待値と一致し、長さが `FLASH_WRITE_LENGTH` のときだけ通す。候補
   リストが再び非空になれば、開始が何であれ拒否する。`fw probe-da` は今後も
   判定を表示するだけで、`da.rs` の定数を書き換えない。**このコードによる
   実機への書き込みはまだ一度も行っていない**ので、書き込み経路そのものは
   未検証のままである（2026-11-15 の手動フラッシュは本コードではない）。

### この経路で担保していないこと（マージ時に「担保済み」と読まない）

以下はコードにあるが、**実機でもフェイクでもテストで固定していない**か、
固定したのは今回で初めてである。前者は未検証として列挙する。

| 項目 | 状態 | 固定するテスト |
|---|---|---|
| `0xD5` の最初セクタ消去失敗 → `0xA5` | 命令列のみ。実機で失敗を起こしていない。フェイクが模す | `a_failed_first_erase_is_a_rejected_error_with_nothing_written`（今回追加） |
| `0xD9` が応答を返さないこと | 命令列のみ（`0x00101920`）。実機で測っていない | `the_restart_does_not_read_a_da_reply`（今回追加） |
| 短い最終パケットのゼロ詰め | 書き込み方向は未検証。§4.4 は読み出し側の直列化から読んだ記述 | `the_final_packet_is_the_short_one_and_is_zero_filled`（今回追加、フェイクが受けた payload を見る） |
| 拒否された `0xD3` が 1 バイト `A5` | 命令列のみ。実機で拒否させていない | `a_refused_range_is_one_byte_and_a_rejected_error` |
| `0xD5` の 2 つの最終 `0x5A` の間にバイトを送るか | 未確定。送らない形だけ実機で通っている | `a_full_write_consumes_exactly_its_replies` |
| DA ビュー ↔ BootROM ビューの写像 | 先頭 64 バイトで確認（2026-11-21、DA `0` = BootROM `0x00200000`）。書き込み後の全範囲読み戻しは未検証 | `the_read_back_address_is_the_bootrom_view_of_the_da_start`（クライアントの式だけ） |
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
   同じゲートは `crates/m241-flash/src/da.rs` の `check_image` 本体に
   `firmware::digest` / `digest::matches` / `sha256`（`sha256_hex` を含む）が
   戻らないことも検査する。`AGENTS.md` §2.3 のバックアップ差分だけが受理規則で
   あり、ベンダーイメージの digest で通す経路は削除済みである（`check_da_image`
   の DA 用 SHA-256 は対象外）。
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
| `cargo tree -p m241-gui --edges normal,build,dev` に `m241-core` と `m241-flash` がある | GUI が書き込みクレートを経由して共有セッションに到達できる（リンクしない状態に戻っていない） |
| `cargo tree -p m241-flash --edges normal,build,dev` に `m241-core` がある | 正しい向きの依存が消えていない |
| `crates/m241-core/Cargo.toml` が `m241-flash` を名指ししない | 解決結果に出ない形の依存（dev 依存など）も塞ぐ。読み取り専用バイナリが書き込みルーチンを持たない |
| `crates/m241-gui/Cargo.toml` が `m241-flash` を名指しし、`device.rs` が `flow::flash(` を呼び、`firmware.rs` が `Confirmation::granted` と `CONFIRMATION` を持つ | GUI の書き込みが共有経路を通り、`WRITE` 入力からしか確認トークンを作らない |
| `crates/m241-gui/src`・`tests` のコード（コメント・文字列を除く）に `OPCODE_*` / `\xNN` / `$PMTK182,6` が無い | GUI が破壊的オペコードを自分で送る第 2 の実装を持たない |
| `set_control_lines` が `crates/m241-core/src/protocol/brom.rs` / `crates/m241-core/src/protocol/port.rs` / `crates/m241-core/src/protocol/simulator.rs` 以外に無い | DTR/RTS を触る場所が増えていない（同名別パスのすり抜けも防ぐ） |
| `crates/m241-flash/src/da.rs` の `check_image` / `all_within` 本体に `firmware::digest` / `digest::matches` / `sha256(_hex)` が無い | strict なバックアップ差分経路に digest 受理が戻らない（差分ヘルパーへ移しても検出。vendor は固定 SHA の別関数 `accept_vendor_image`、`check_da_image` の DA 用 SHA-256 は対象外） |
| `crates/m241-flash/src/da.rs` の `pub fn write` / `write_vendor` が本体のトップレベル（brace depth 1）で `write_range` / `write_range_accepted` より前に `check_layout()?` を含む | 開始アドレスのゲートが書き込み経路から外れていない（コメント・別関数・未呼び出しの入れ子関数では通らない。`write_range(_accepted)` に到達する公開関数を名前ではなく呼び出しで探す） |
| `crates/m241-flash/src/da.rs` の `DaDigest::for_test_da` の直前が `#[cfg(any(test, feature = "simulator"))]` である | DA の期待ダイジェストが type で pin されている状態を保つ（この属性が外れると出荷ビルドが任意の digest のトークンを構築でき、`AGENTS.md` §2.5 の「ベンダー DA のみ」が型の性質でなくなる。通常ビルドの唯一の構築子は `DaDigest::vendor()`） |

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
- **書き込み手順そのものの正しさ:** 範囲定数の値そのものの妥当性、`start = 0`
  が正しいことの根拠（`docs/dev/05-verification.md` §1.2）、読み戻し照合の有無、
  失敗時のリカバリ案内は `tools/check_safety.py` の対象外であり、上の 2 つの
  レビュー節とテストで担保する。同ファイルの `write` / `write_vendor` が
  `check_layout()` を呼ぶこと自体は機械的に検査するが、そのゲートの中身は検査しない。

## 書き込み可能な実行（`fw flash --yes` / `fw flash --recover --yes` / `fw flash --allow-vendor --yes` / `fw probe-da --yes`）の承認（2026-11-21 変更）

`fw flash --yes`（`--recover` / `--allow-vendor` を含む）と `fw probe-da --yes` の承認は原則として
ユーザー本人が行う。その
セッションまたは指示でユーザーが明示的にサブエージェントへ承認権限を委任した場合
**に限り**、対象の実行を準備したセッションとは独立したサブエージェントが `--yes`
付与可否を判定できる。破壊的変更に事前の人間承認を要する旨の明示的なユーザー指示が
ある場合は、常にそちらが優先する。確認項目・拒否条件・記録手順は `AGENTS.md` §2.5
を正本とする。

`fw probe-da --yes` は、クライアントが `0xD3` / `0xD4` / `0xD5` / `0xD9` を送らなくても
対象に含む。DA を RAM にロードし `0xA8` で**実行**するため、ロードされた DA 自身が
フラッシュを消去・書き込みできる。`check_da_image` はサイズ（14,896 バイト）・
先頭 4 バイト（`00 00 A0 E1`）・**全文 SHA-256** を照合し、いずれかが合わない
`--da` はポートを開く前に拒否する（W2a）。承認でも `--da` の**完全な** SHA-256 が
ベンダー値
`190d7064cf2320752d8eb65681142730b21cddc90e0d1491d4e8379c3f2e89aa`
（V1.12 / V1.13 の公式パッケージで同一、14,896 バイト）と一致することを確認し、
一致しない DA は承認不可とする。

- **独立性と差し戻し禁止**: 承認者は対象の実行を準備したセッションと別であり、準備
  セッションから fork した履歴を持ってはならない。準備側の要約・結論・説明は渡さず、
  生の成果物とコマンドログのみを検証させる（[`09-review-protocol.md`](09-review-protocol.md)）。
  拒否された場合に原因を直さず別の承認者へ回すことは禁止し、原因修正後に新しい
  成果物からやり直す。
- **客観的な成果物検証（`fw flash`）**: 承認者は、現セッションの `device info`・
  `fw dump`・`fw probe-da` の生ログで装置の同一性（`B-core_1.20`、`01017-00F`、
  フラッシュ `00C2:22BA`）と解決済みの具体的なシリアルポート（`--port auto` は不可）
  を確認し、バックアップ・書き込みイメージ・`--da`（`DA_v3.8.4.bin`）の SHA-256 を
  自ら再計算する。`--da` の値は上記ベンダー値と完全一致しなければならない。
  イメージは、対応プロファイル（現状 V1.13 `01017-00F`、長さ `0x6345C`）の
  自機バックアップと同一、または `PATCH_OFFSETS`（`0x402F6` / `0x402F7` / `0x59C2C`）
  **のみ**が異なること（それ以外の差分 0 バイト）を `AGENTS.md` §2.3・
  `da.rs::check_image` に沿って自ら確認する。
- **`fw probe-da` の承認**: `fw probe-da --yes` では、装置の同一性、解決済みポート、
  `--da` の完全な SHA-256（上記ベンダー値と一致）、および完全なコマンド行を確認する。
  イメージ・バックアップ・書き込み範囲・読み戻しの項目は不要だが、DA の実行を伴う
  ため独立レビューと記録は省略しない。
- **範囲確定・読み戻し・復旧手順・コマンド行**: `FLASH_WRITE_START` が
  [`05-verification.md`](05-verification.md) §1 の実機計測で確定済みであり、
  `crates/m241-flash/src/da.rs` の `UNRESOLVED_STARTS` が空であること（未確定なら
  拒否）、全イメージの読み戻し照合計画、復旧手順（`da.rs` に沿い `0xD9` 送信後に応答を
  待たず再起動または電源再投入し、`--assume-brom` または 4 探査同期で BootROM に再入して
  自機バックアップを書き戻す／部分書き込み状態からログ取得しない）、および `--yes`・
  解決済み `--port`・`--da`・イメージ・バックアップを含む完全なコマンド行を確認する。
- **`fw flash --recover` の承認**: `--recover` は部分書き込み後のユニットに自機
  バックアップを書き戻す唯一の経路で、`verify_device`（全 span 比較）を意図的に
  省略する。承認者は、イメージがバックアップと**完全一致**（差分 0 バイト）であり、
  パッチ済みイメージではないこと、ベクタ / family のマーカー検査は残ることを確認する。
  イメージが 1 バイトでも違えば拒否する。通常の `fw flash --yes` では
  `verify_device` が必須のままである。固定するテストは
  `flash_recover_writes_the_backup_without_the_whole_unit_comparison`、
  `flash_recover_refuses_an_image_that_is_not_the_backup`、
  `flash_refuses_a_unit_that_does_not_hold_the_backup_before_any_write_opcode`。🧪
- **`fw flash --allow-vendor` の承認**: `--allow-vendor` は公式 V1.13 ベンダー
  イメージ（長さ 406,620、SHA-256
  `32fe52690db9947bdbfa58caa6dba450b3f4222b23f2e045f606252649159435`）だけを
  受理する別ゲートで、strict な `check_image` のバックアップ差分を置き換えない。
  承認者は固定 SHA-256 の完全一致、`--backup` とユニット同一性、`verify_device` が
  スキップされないこと、読み戻しがベンダーイメージと比較されること、個体データ
  14 バイトが上書きされることを確認する。V1.12 や別ビルドは拒否する。固定する
  テストは `the_vendor_gate_accepts_only_the_pinned_v1_13_digest`、
  `flash_allow_vendor_writes_the_vendor_image_and_reads_it_back`、
  `flash_allow_vendor_still_requires_the_unit_to_match_the_backup`、および
  `M241_VENDOR_IMAGE` を渡した dry-run 警告テスト。🧪
- **記録主体の分離と永続記録**: 承認者はポートを開かず、準備ブランチも編集しない。
  `--yes` 実行**前**に承認者自身のセッションから直接、**永続的な**判定記録
  （承認者が投稿する PR コメント、または承認者が準備とは別のブランチ / PR で
  コミットするレビュー記録）を残す。会話ログやセッションの要約だけを記録にしない。準備
  セッションが承認記録を代筆・編集してはならず、事前承認は
  [`05-verification.md`](05-verification.md) §1（事後の実機計測専用）へ書かない。
- **`log erase` は対象外**: ユーザーの履歴データを消すため、引き続きユーザー自身の
  指示を要する。

`Confirmation`（`Confirmation::of_command_line(yes)`）は呼び出し側で `--yes` が
渡されたことだけを表すトークンであり、承認記録やセッション ID を機械的に検証する型では
ない（手続き上のゲートは `AGENTS.md` §2.5 が担う）。
