# 安全性レビュー

読み取り（`m241-core`）と書き込み（`m241-flash`）のクレート分離が保証する範囲と限界、およびレビュー観点を定義する。`✓` は `tools/check_safety.py` による機械検査、`👁` は人／エージェントによる差分確認を表す。

## なぜ分けるのか

シミュレータは正しさの証明ではなく回帰ピンにすぎない（[`05-verification.md`](05-verification.md) §3）。`m241-core` に破壊的コマンド（`$PMTK182,6`, `0xD3`/`0xD4`/`0xD5`/`0xD9`）を持たせないことで、`m241-core` のみリンクするバイナリが書き込み・消去ルーチンを誤って呼ぶ事故を依存グラフで防ぐ。

ただし、`m241-flash` と共有する低レベルプリミティブ（`Port::open`、`Transport::write_all`、`pmtk::line`、`Frame`）は公開しているため、コンパイルだけで生バイト送信までは防げない。その補完を `tools/check_safety.py` と本ページのレビュー観点が担う。

## クレート構成と GUI 書き込み（2026-11-23 変更）

```text
m241-core（読み取り専用）←── m241-flash（書き込み）
        ↑                          ↑
   m241-gui ───────────────────────┘
        └── m241-cli ──────────────┘
```

- **`m241-core`**: 読み取り・解析・パッチ計画のみ。破壊的ルーチンを持たず、`m241-flash` への依存は禁止。
- **`m241-flash`**: `$PMTK182,6`（`crates/m241-flash/src/erase.rs`）と Download Agent（[`01-da-protocol.md`](../reference/01-da-protocol.md)）。
- **`m241-cli` / `m241-gui`**: 両方に依存。ユーザー指示（2026-11-23）により GUI も `m241_flash::flow::flash` 経由で書き込み・復旧・`probe-da`・`da-restart` を実行できる。
- **GUI ゲートの限界**: 旧ルール（依存グラフ禁止）より意図的に弱いテキスト検査であり、チェッカー改変を伴う変更は防げない。構造的保証は `m241-flash` 側（固定範囲、`check_layout`、`check_image`、`verify_device`、全長読み戻し）に残る。GUI は `--yes` 相当を持たず、`WRITE`/`PROBE`/`RESTART` の手入力を要し、吸い出しは CLI と同じ 2 回読み＋ハッシュ比較を行う。GUI からの実機書き込みは**未検証**（シミュレータのみ）。

## 書き込み経路のレビュー（`m241-flash` 対象・必須）

1. **消去範囲がハードコード定数か（👁）**: `crates/m241-flash/src/da.rs` の `FLASH_WRITE_START`（`0`）/ `FLASH_WRITE_LENGTH`（`0x6345C`）リテラルのみ。ユーザー入力や応答から算出しない。`FLASH_WRITE_END = start + length - 1`。
2. **開始アドレスと `check_layout`（👁）**: 2026-11-21 実機読み出し（`05-verification.md` §1.2）で DA 側基底 `0x00000000` が確定し、`UNRESOLVED_STARTS` は空。`check_layout` は「候補空・期待開始 `0`・開始一致・長さ一致」をすべて要求する。厳格経路は `check_image` がバックアップ差分をパッチ位置のみに限定し、CLI は `verify_device` を先行実行する（ライブラリ単体の `write` は呼ばない）。
3. **書き込み後の読み戻し照合（👁）**: `PendingWrite::verify_written` が `0xD9` 送信→BootROM 再入（NMEA → `$PMTK180*3B` → 115200）→`m241-core` の `MtkBootRom` で `DUMP_CHUNK` 分割読み戻しを行い一致を検証する（再入失敗と不一致を区別）。
4. **失敗時の復旧案内（👁）**: `0xD5` 後の失敗は `PartialWrite::recovery` で書込済みバイト数・`0xD9`（無応答を待たない）・`--assume-brom`・`fw flash --recover --yes`（バックアップ完全一致のみ許可、`verify_device` 省略）を案内する。
5. **全定数の citation（👁）**: 全定数が `01-da-protocol.md` または `03-hardware-facts.md` を指すこと（`REPLY_TIMEOUT` 等プロトコル外定数はその旨明記）。

### DA ロードと `fw probe-da`
- `DaLoader::load` は RAM への `0xA1` と `0xA8`（`0x00100800`）のみ送信し、フラッシュを書かない（`0xD3`/`0xD4`/`0xD5`/`0xD9` 不使用）。ただし実行中の DA は書き込み可能なため `AGENTS.md` §2.5 と `DaDigest::vendor()`（14,896 B、先頭 `00 00 A0 E1`、全文 SHA-256 `DA_IMAGE_SHA256`）を必須とする。
- 実 DA ファイルによる一致テスト（`the_vendor_da_matches_the_pinned_digest_and_loads_when_the_vendor_da_is_available`）はベンダーファイルがある環境でのみ実行（CI では `eprintln!` で skip）。pin 値自体は CI 常時実行のリテラル回帰テストで固定する。
- `fw probe-da` の読み出し対象は `PROBE_ADDRESSES`（`0`, `0x00200000`）の 2 候補固定。

### テストのみで未検証／未担保の項目
`0xD5` 初回セクタ消去失敗 `0xA5`、`0xD3` 拒否の 1 バイト `A5`、`0xD5` 末尾 2 個の `0x5A` 間の規則、DA 側最終チェックサム（偽 DA 未モデル化）、および GUI からの実機書き込みは未検証（CLI の実機書き込み結果は `05-verification.md` §1.5）。

## 読み取り経路のレビュー（`m241-core` 対象・必須）

1. **制御線（DTR/RTS）の保護（✓ + 👁）**: `set_control_lines` は `brom.rs` / `port.rs` / `simulator.rs` の完全パス限定。本番呼出は必ず `StreamState::Silent` の後ろに置く（現在本番呼出 0 件）。
2. **破壊的コマンド・語彙の不在（✓ + 👁）**: `m241-core` のコードに `$PMTK182,6`, `0xD3`/`0xD4`/`0xD5`/`0xD9`（`\xNN` 含む）、`erase*`/`flash_write`/`program*`（`erased` 除く）が無いこと。算術・動的文字列による迂回が無いことを 👁 で確認。
3. **記録再開の保証（👁）**: `log_stop` 後は失敗経路でも `log_resume` を実行する（`logging_is_resumed_even_when_the_transfer_fails`）。
4. **タイムアウト上限（👁）**: `read_line`/`read_exact` に締切があり、`Error::is_fatal` が `false` なのはタイムアウトのみ。

## 機械的な担保（`tools/check_safety.py`）

`./tools/check.sh` の `safety` ゲートとして毎回実行し、`crates/m241-flash/tests/safety_gate.rs` で違反検出と除外の妥当性を自動検証する。

| 検査 | 守る不変条件 |
|---|---|
| `m241-core` の語彙・オペコード・依存検査 | `m241-core` に破壊的コマンド／語彙／`m241-flash` 依存（dev/build 含む）が入らない |
| `m241-flash` の依存検査 | `m241-flash` → `m241-core` の向きを維持 |
| `m241-gui` の依存・経路・オペコード検査 | `m241-gui` は `flow::flash(` を呼び、`Confirmation::granted` / `Job::Flash` を許可ファイルに限定し、GUI コードに破壊的オペコード（裸リテラル・`\xNN`・PMTK 消去）を持たない |
| `m241-app` の分離検査 | `APP_MAY_LINK_FLASH = False` の間 `m241-flash` 依存・破壊的コマンド（Rust/TS）・複数箇所の `Port::open` を拒否 |
| `set_control_lines` パス限定 | BootROM 入口・port・simulator の完全パス外での使用を拒否 |
| `da.rs` の 3 検査 | `check_image`/`all_within` 内の digest 迂回禁止、`write`/`write_vendor` のトップレベルで `check_layout()?` 先行呼出、`DaDigest::for_test_da` 直前の `#[cfg(any(test, feature = "simulator"))]` |

**限界**: 算術組み立て（`0xD0 + 5`）、動的文字列生成、公開低レベル API による意図的な生送信、および書き込み手順・範囲値自体の正しさは静的検査では防げず、上のレビューとテストで担保する。

## 書き込み可能な実行の承認

`fw flash --yes`（`--recover` / `--allow-vendor` 含む）および `fw probe-da --yes` の承認は原則ユーザー本人が行い、明示的委任がある場合のみ独立した非 fork レビュアーが事前承認できる。確認項目・拒否条件・永続記録の要件は `AGENTS.md` §2.5 を正本とする（`log erase` は対象外）。`Confirmation` は `--yes` 受領を表すトークンであり、承認記録を型で証明するものではない。
