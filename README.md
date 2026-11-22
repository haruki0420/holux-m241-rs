# Holux M-241 の日付が 19.6 年ずれる問題を直す（holux-m241-rs）

Holux M-241 は MediaTek MT3318 を積んだ GPS ロガーである。2006 年製のファームウェアは、
GPS エポックから Unix 時刻への変換に使う定数 `315964800` を直書きしており、2019 年 4 月 6 日に
起きた 2 回目の GPS ウィークナンバーロールオーバーを考慮していない。そのため、記録される
日時はすべて 19.6 年（7168 日）過去になる。このツールは、`0x402F4` にある 4 バイトの定数を
`315964800` から `935280000` に置き換えてこのずれを直し（イメージ全体で実際に値が変わるのは
3 バイト）、あわせて記録した軌跡を端末から吸い出して地図に出す。

**状態: 1.0.0（初回公開リリース）。** 読み取り系（ファームの吸い出し、ログの取得と変換、
パッチ適用、NMEA の日付補正）と書き込み系（`fw flash`）の主要経路は実機の M-241 で動いて
いる。2026-11-22 のセッションで、復旧書き込みと strict 書き込みの 2 回が 1589 パケット
完走し、406,620 バイトの全長読み戻しが一致した。パッチ済み V1.14 の起動も確認済みで、
NMEA の既定日付はパッチ済みエポックの `22-08-99` になる。`log erase` は 2026-11-21 の
実機セッションで動作を確認している（その後の変更は未検証、`docs/dev/05-verification.md` §1a）。

一方で、**測位ありの日付、GUI の実機動作（接続・追従・JPEG 書き込み）、macOS、Bluetooth
SPP、他のユニット形状は未検証**である。ベンダー公式イメージの書き込みは完走するものの、
起動したファームが個体データ 4 バイトを復元するため、現在の読み戻し照合では一致しない
（設計課題）。何が検証済みで何が未検証かは
[`docs/dev/05-verification.md`](docs/dev/05-verification.md) に列挙してある。

## 機能

| やること | コマンド |
|---|---|
| 端末のシリアルポートを探す | `m241 device ports` |
| 端末の識別情報を読む | `m241 device info` |
| 環境とケーブルを診断し、ベンダーファイルを取り込む | `m241 device doctor` |
| ファームウェアを吸い出す（読み取りのみ、安全） | `m241 fw dump` |
| PC 上で、吸い出したイメージに日付修正を当てる | `m241 fw patch` |
| 修正したイメージを書き戻す（破壊的） | `m241 fw flash` |
| DA 側のアドレス空間を読む（書き込みなし） | `m241 fw probe-da` |
| DA が RAM に残った端末をソフトで再起動する（書き込みなし） | `m241 fw da-restart` |
| 軌跡ログの情報を表示する | `m241 log info` |
| 軌跡ログを端末から吸い出す | `m241 log download` |
| 軌跡ログを GPX / KML / CSV / NMEA / JSON に変換する | `m241 log read` |
| 軌跡ログを分割・結合・地点削除して書き出す | `m241 log edit` |
| 軌跡ログを消去する（破壊的・バックアップ必須） | `m241 log erase` |
| 写真の EXIF に軌跡の座標を書き込む | `m241 log geotag` |
| 軌跡と写真を 1 つの KMZ にまとめる | `m241 log kmz` |
| 壊れた NMEA の日付を修復する（書き換えなし） | `m241 nmea repair` |
| 書き換えずに、流れている NMEA の日付を補正して中継する | `m241 nmea proxy` |

GUI（`m241-gui`）は同じ仕事を 4 画面（接続 / ログ / ファーム / 設定）で提供する。
呼ぶのは CLI と同じコア関数で、別実装ではない。GUI は実機では未検証である。

## インストール

### リリースバイナリ

`v*` タグの push で GitHub Release が作られ、Ubuntu / Windows / macOS のバイナリと
SHA-256 チェックサム（`SHA256SUMS.txt`）が添付される。リリースページ
（<https://github.com/haruki0420/holux-m241-rs/releases>）から自分の OS のファイルを取る。

| OS | CLI | GUI |
|---|---|---|
| Linux (x86_64) | `m241-linux-x86_64` | `m241-gui-linux-x86_64` |
| Windows (x86_64) | `m241-windows-x86_64.exe` | `m241-gui-windows-x86_64.exe` |
| macOS (aarch64) | `m241-macos-aarch64` | `m241-gui-macos-aarch64` |

取得したファイルは `SHA256SUMS.txt` と照合する（例: `sha256sum -c SHA256SUMS.txt`）。
インストーラは無く、`PATH` の通った場所に置けば動く（実行ビットが落ちている場合は
`chmod +x`）。Linux では udev（`libudev`）が、GUI では X11 / Wayland と OpenGL の
ライブラリが実行時に要る。端末にアクセスするユーザーは `dialout` グループに入れる
（`sudo usermod -aG dialout $USER` の後、ログインし直す）。OS ごとの詳細は
[`docs/user/01-install.md`](docs/user/01-install.md) にある。

### ソースからビルド

```bash
cargo build --release     # target/release/m241 と target/release/m241-gui
cargo test --workspace
./tools/check.sh          # fmt / clippy / test / docs リンク / design / safety の 6 ゲート
```

Rust 1.95 以降が必要（`Cargo.toml` の `rust-version`）。Linux でビルドする場合は、
`serialport` が `libudev` を、GUI が X11 / Wayland と GL をリンクするため、次の
パッケージが要る（CI と同じ）。

```bash
sudo apt-get install -y --no-install-recommends \
  libudev-dev \
  libxkbcommon-dev libwayland-dev libx11-dev libxi-dev \
  libgl1-mesa-dev libegl1-mesa-dev pkg-config
```

端末に触れるのは `serialport` クレート経由だけである。オフラインのコマンドはシリアル
ポートを開かない。もう 1 つの依存は `image` で、KMZ の長辺縮小
（`kmzMaxPhotoEdgeSize`）のための JPEG 復号・符号化にだけ使い、feature は `jpeg` のみを
有効にしている（PNG 等の他の形式はビルドしない）。
`m241-core` から見た `image` のビルド feature が `jpeg` だけであることは
`cargo tree -p m241-core -e normal -f '{p} {f}'` で確認できる。

### 使い方（短い例）

```bash
# ケーブルを挿し、どのポートか確認する（--port auto が既定）
m241 device ports
m241 device info

# まず自分のファームウェアを吸い出して保管する。2 回読んでハッシュ一致を要求する
m241 fw dump -o backup/firmware.bin

# 吸い出したイメージに日付修正を当てる。PC 上だけで完結し、端末には触れない
m241 fw patch backup/firmware.bin -o backup/firmware-v114.bin

# 軌跡を吸い出して GPX にする
m241 log download -o backup/log.bin
m241 log read backup/log.bin --format gpx -o track.gpx
```

書き込み（`m241 fw flash`）は破壊的である。実行前に `m241 fw flash --help` と
[`DISCLAIMER.md`](DISCLAIMER.md) を読み、自分で吸い出したバックアップを用意する。
GUI は `m241-gui` で起動する。

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
`check_layout` で検査する。`fw flash` は 2026-11-22 に実機で書き込みと全長の読み戻しまで
通っている（[`docs/dev/05-verification.md`](docs/dev/05-verification.md) §1.5）。ただし、
ベンダー公式イメージの書き込みは読み戻しが一致しない設計課題を抱えている。読み取り専用の
`m241-core` は書き込み・消去ルーチンを持たない。

どこから何を取り入れたか、どの主張がまだ仮説かは
[`docs/dev/04-prior-art.md`](docs/dev/04-prior-art.md) に書いてある。

## 安全について

JTAG を持たない、生産終了から年数が経つ機器のファームウェアを書き換えるツールである。
書き込む前に [`DISCLAIMER.md`](DISCLAIMER.md) を読むこと。まず自分でファームウェアを吸い出して
保管し、書き込むのはそこから作ったイメージだけにする。

## ドキュメント

| ページ | 何が分かるか |
|---|---|
| [`CHANGELOG.md`](CHANGELOG.md) | リリースごとの変更点と、既知の未検証項目 |
| [`docs/user/01-install.md`](docs/user/01-install.md) | OS ごとの導入、ポートの見つけ方、ベンダーファイルの用意 |
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

コードとドキュメントは MIT（[`LICENSE`](LICENSE)）。プロプライエタリなバイナリの権利は、
それぞれの所有者に帰属する。書き込みを行う前に [`DISCLAIMER.md`](DISCLAIMER.md) を読むこと。
