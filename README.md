# Holux M-241 の日付が 19.6 年ずれる問題を直す（holux-m241-rs）

Holux M-241 は MediaTek MT3318 を積んだ GPS ロガーである。2006 年製のファームウェアは、
GPS エポックから Unix 時刻への変換に使う定数 `315964800` を直書きしており、2019 年 4 月 6 日に
起きた 2 回目の GPS ウィークナンバーロールオーバーを考慮していない。そのため、記録される
日時はすべて 19.6 年（7168 日）過去になる。このツールは、`0x402F4` にある 4 バイトの定数を
`315964800` から `935280000` に置き換えてこのずれを直し（イメージ全体で実際に値が変わるのは
3 バイト）、あわせて記録した軌跡を端末から吸い出して地図に出す。

**状態: 開発中。** 読み取り系（ファームの吸い出し、ログの取得と変換、パッチ適用、NMEA の
日付補正）は実装済みで、読み取り経路（`fw probe-da` による DA 側の読み出し）は実機の
M-241 で動いた。`log erase` も 2026-11-21 の実機セッションで動作を確認している（その後の
変更は未検証、`docs/dev/05-verification.md` §1a）。書き込み系（`fw flash`）は DA 経由の
書き込み・`0xD9` 後の BootROM 再入・読み戻しまで配線済みだが、このリポジトリのコードに
よる実機への書き込みは未検証である。何が未検証かは
[`docs/dev/05-verification.md`](docs/dev/05-verification.md) に列挙してある。

## 提供予定の機能

| やること | コマンド |
|---|---|
| 端末のシリアルポートを探す | `m241 device ports` |
| 端末の識別情報を読む | `m241 device info` |
| ファームウェアを吸い出す（読み取りのみ、安全） | `m241 fw dump` |
| PC 上で、吸い出したイメージに日付修正を当てる | `m241 fw patch` |
| 修正したイメージを書き戻す | `m241 fw flash` |
| DA が RAM に残った端末をソフトで再起動する（書き込みなし） | `m241 fw da-restart` |
| 軌跡ログを端末から吸い出す | `m241 log download` |
| 軌跡ログを GPX / KML / CSV / NMEA / JSON に変換する | `m241 log read` |
| 写真の EXIF に軌跡の座標を書き込む | `m241 log geotag` |
| 書き換えずに、流れている NMEA の日付を補正して中継する | `m241 nmea proxy` |

GUI（`m241-gui`）は同じ仕事を 4 画面（接続 / ログ / ファーム / 設定）で提供する予定である。
呼ぶのは CLI と同じコア関数で、別実装ではない。

## ビルド方法

```bash
cargo build --release
cargo test --workspace
./tools/check.sh          # fmt / clippy / test / docs リンク / design / safety の 6 ゲート
```

Rust 1.95 以降が必要（`Cargo.toml` の `rust-version`）。端末に触れるのは `serialport`
クレート経由だけである。オフラインのコマンドはシリアルポートを開かない。もう 1 つの依存は
`image` で、KMZ の長辺縮小（`kmzMaxPhotoEdgeSize`）のための JPEG 復号・符号化にだけ使い、
feature は `jpeg` のみを有効にしている（PNG 等の他の形式はビルドしない）。
`m241-core` から見た `image` のビルド feature が `jpeg` だけであることは
`cargo tree -p m241-core -e normal -f '{p} {f}'` で確認できる。

## このリポジトリに含まれないもの

Holux のファームウェアイメージ、ダウンロードエージェント（`DA_v3.8.4.bin`）、ベンダーの DLL、
ベンダーの実行ファイルはプロプライエタリであり、このリポジトリでは再配布していない。これらの
ベンダーファイルは利用者が用意したローカルのフォルダから読み込む。`.gitignore` がよくある
ファイル名を塞いでいるため、誤って `git add .` しても公開されない。

`DA_v3.8.4.bin` を必要とするのはファームウェアの書き込み（`m241 fw flash`）だけで、端末の
読み取りや PC 上でのパッチ適用には要らない。入手方法と置き場所は
[`docs/user/01-install.md`](docs/user/01-install.md) に書いてある。

## フラッシュ処理はバイナリの逆解析から作り直す

ダウンロードエージェントのコマンドの意味づけについて、既存の Python 実装 2 つは互いに
食い違っており、どちらも同じ不完全な理解の上に書かれている。どちらかを移植するのではなく、
このプロジェクトは `DA_v3.8.4.bin` を逆アセンブルし、コマンド体系を
[`docs/reference/01-da-protocol.md`](docs/reference/01-da-protocol.md) に記録してから実装する。
ダウンロードエージェントのドライバは読み取り側とは別クレートの
`crates/m241-flash/src/da.rs` にあり、書き込み範囲は DA 側の開始アドレス `0` に固定して
`check_layout` で検査する。`fw flash` は CLI に配線済みだが、実機への書き込みは
未検証である。読み取り専用の `m241-core` は書き込み・消去ルーチンを持たない。

どこから何を取り入れたか、どの主張がまだ仮説かは
[`docs/dev/04-prior-art.md`](docs/dev/04-prior-art.md) に書いてある。

## 安全について

JTAG を持たない、生産終了から年数が経つ機器のファームウェアを書き換えるツールである。
書き込む前に [`DISCLAIMER.md`](DISCLAIMER.md) を読むこと。まず自分でファームウェアを吸い出して
保管し、書き込むのはそこから作ったイメージだけにする。

## ドキュメント

| ページ | 何が分かるか |
|---|---|
| [`docs/dev/03-hardware-facts.md`](docs/dev/03-hardware-facts.md) | 実測した定数すべてと、その測定方法 |
| [`docs/dev/05-verification.md`](docs/dev/05-verification.md) | 何を検証済みで、何が未検証か |
| [`docs/dev/04-prior-art.md`](docs/dev/04-prior-art.md) | どこから何を持ってきたか |
| [`docs/dev/06-open-questions.md`](docs/dev/06-open-questions.md) | まだ答えの出ていない問い |
| [`docs/dev/07-eztour-spec.ja.md`](docs/dev/07-eztour-spec.ja.md) | 公式 ezTour v3.0 の振る舞い仕様（クリーンルーム再実装の基準） |
| [`docs/dev/11-safety-review.md`](docs/dev/11-safety-review.md) | 読み取りと書き込みの分離、書き込み・読み取り経路のレビュー観点 |
| [`docs/reference/01-da-protocol.md`](docs/reference/01-da-protocol.md) | `DA_v3.8.4.bin` 逆解析による Download Agent プロトコル仕様 |
| [`docs/reference/02-cli.md`](docs/reference/02-cli.md) | コマンド体系と終了コード |
| [`DESIGN.md`](DESIGN.md) | 画面の設計（方向、トークン、各画面） |
| [`PRODUCT.md`](PRODUCT.md) | 誰のためのもので、何をしてはいけないか |
| [`AGENTS.md`](AGENTS.md) | 自動化されたコントリビューター向けの作業メモ |

## ライセンス

コードとドキュメントは MIT。プロプライエタリなバイナリの権利は、それぞれの所有者に帰属する。
