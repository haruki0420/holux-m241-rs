# Holux M-241 の日付が 19.6 年ずれる問題を直す（holux-m241-rs）

Holux M-241 は MediaTek MT3318 を積んだ GPS ロガーである。2006 年製のファームウェアは、
GPS エポックから Unix 時刻への変換に使う定数 `315964800` を直書きしており、2019 年 4 月 6 日に
起きた 2 回目の GPS ウィークナンバーロールオーバーを考慮していない。そのため、記録される
日時はすべて 19.6 年（7168 日）過去になる。このツールは、`0x402F4` にある 4 バイトの定数を
`315964800` から `935280000` に置き換えてこのずれを直し（イメージ全体で実際に値が変わるのは
3 バイト）、あわせて記録した軌跡を端末から吸い出して地図に出す。

**状態: 1.0.0（初回公開リリース）。** 実機の M-241 で動いているのは、読み取り系
（`device info` / `fw dump` / `fw probe-da`、ログの取得と変換）、PC 上で完結するパッチ適用
（`fw patch`）、書き込み系（`fw flash`）である。書き込みは 2026-11-22 のセッションで、
復旧書き込みと通常の書き込み（自機バックアップとの差分だけを許す経路）の 2 回が
1589 パケット完走し、406,620 バイトの全長読み戻しが一致した。パッチ済み V1.14 の起動も
確認済みで、NMEA の既定日付はパッチ済みエポックの `22-08-99` になる。ただし、実機で動いた
バイナリは [`docs/dev/05-verification.md`](docs/dev/05-verification.md) §1.5 に記録された
`b8d91ee` 時点のもので、その後の flash / CLI の変更（DA ダイジェストゲートなど）は実機で
実行していない。`log erase` は 2026-11-21 の実機セッションで動作を確認している（その後の
変更は未検証、同ページ §1a）。ログの解析は、ブロック間に混在する設定ジャーナルをレコードと
誤読しないよう PR #34 で直した（保存済みダンプの再解析で、2026-11-21 は 10,942 点・
チェックサム不一致 0、書き込み後の 2 ダンプは 0 点・設定ジャーナルのみ。修正後のバイナリは
実機で走らせていない）。

NMEA については、壊れた NMEA の日付を直す `nmea repair` はオフラインの変換処理として
実装済みで、実機のストリームを中継する `nmea proxy` は 2026-11-22/23 に実機の live
ストリームで動いた（stdout と TCP `--bind`。TCP は 9 文を受信し、チェックサムは 9/9 有効、
`--offset-days 1` で日付を補正した）。長時間連続・複数クライアント・非ループバック bind・
fix ありストリームは未検証である。

一方で、**測位ありの日付、GUI からの実機書き込み、GUI の追従描画・GUI 経由の JPEG 書き込み、
GUI の背景地図（OpenStreetMap タイル）の Wayland 実画面での見た目、macOS、Bluetooth SPP、
`nmea proxy` の長時間連続・複数クライアント・非ループバック bind・
fix ありストリーム、他のユニット形状は未検証**である。GUI の接続画面と「接続して確認」は
2026-11-22 に実機で成功しており（読み取り専用）、`B-core_1.20` / `0043` / `01017-00F` /
SDK 1.0 とログ使用量 3,292 B を表示した。ベンダー公式イメージの書き込みは
完走するものの、起動したファームが個体データ 4 バイトを復元するため、現在の読み戻し照合
では一致しない（設計課題）。個体データは 14 バイト（`0x5C`–`0x5F` の 4、`0xF8`–`0xFD` の
6、`0x63458`–`0x6345B` の 4）である。このうち起動時にファームが書き戻すのは `0x5C`–`0x5F`
の 4 バイトで、実測で読み戻しに差が出たのはそのうち `0x5C` の 1 バイトだけだった。何が
検証済みで何が未検証かは
[`docs/dev/05-verification.md`](docs/dev/05-verification.md) §2 に列挙してある。

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
| GUI からファームウェアを吸い出し・パッチ・書き込みする | `m241-gui`（ファーム画面） |
| GUI からログを吸い出し・プロジェクト保存・NMEA 修復/中継する | `m241-gui`（ログ画面の「ログ操作」） |
| GUI で軌跡を OpenStreetMap の地図に重ねる | `m241-gui`（ログ画面） |

GUI（`m241-gui`）は同じ仕事を 4 画面（接続 / ログ / ファーム / 設定）で提供する。
呼ぶのは CLI と同じ関数で、別実装ではない。ファーム画面は吸い出し（2 回読み＋ハッシュ
一致）・パッチ・書き込み・復旧・読み戻しまで行い、書き込みは `m241_flash::flow` の
共有セッションを通る。`WRITE` はユーザー自身が入力する（`--yes` 相当の省略経路はない）。
ログ画面はログの吸い出し・プロジェクト保存・NMEA 修復/中継・GPX を含む読み書きと、
OSM タイルの背景地図を持つ。接続画面と「接続して確認」は 2026-11-22 に実機で成功した
（読み取り専用）。Log 画面の実ログ表示と地図は 2026-11-23 に開発機の実ウィンドウ
（X11）で確認した（M-241 には接続していない）。GUI からの実機書き込み・追従描画・
GUI 経由の JPEG 書き込みは未検証である。

Log 画面の地図は OpenStreetMap のタイルを背景に描き（投影はタイルと同じ Web Mercator）、
タイルは OS のキャッシュに保存する（30 日で失効、200 MB 上限、可視範囲＋1 リングのみ、
同時 4 取得。失敗したタイルは 60 秒後に再試行する）。オフライン時は模式キャンバスに戻り
「地図タイルを取得できません（オフライン）」と表示するが、軌跡・点の選択・再生・書き出しは
そのまま使える。帰属表示 `© OpenStreetMap contributors` は OSM タイルを描いたフレームに
だけ出す（見本・オフライン・読み込み中は出さない）。ウィンドウの有無は
`native_pixels_per_point` で判定するため、Wayland でも実ウィンドウなら取得する
（Wayland は `inner_rect` を返さないため、旧判定では地図が動かなかった）。
`--screenshot` はネットワークを使わず、見本フレームには
「見本：地図タイルは表示しません」、ウィンドウの無い描画には「ヘッドレス」と出る。
実ウィンドウでのタイル取得は Wayland（Hyprland / winit Wayland バックエンド）と
X11（Xwayland）で 2026-11-23 に確認した（キャッシュに書かれた実タイル数で確認）。
タイルと軌跡の整合は X11 の実ウィンドウ画像で確認済みである。Wayland の実画面での
見た目は、この環境では screencopy が応答せず未検証。タイルの取得は 1 枚だけ
2026-11-23 に手動でも確認した（43,806 バイトの 256×256 PNG を取得・デコード）。

## ネットワーク

地図タイルは既定で ON で、Log 画面に表示中の範囲のタイルを OpenStreetMap
（<https://www.openstreetmap.org/copyright>）から取得する。表示範囲は軌跡に
追従するので、**地図を出している間は、どのあたりの地図を見ているかが
OpenStreetMap に伝わる**。軌跡のファイルや座標列そのものを送ることはないが、
表示範囲から滞在場所は推測されうる。プライバシーが気になる場合は、設定画面の
「地図タイルを使う」を OFF にする。オフにすると地図は模式キャンバスに戻り、
軌跡・再生・選択・書き出しはそのまま使える。

タイルは地図の描画にだけ使い、タイルを描いたフレームには
[OpenStreetMap の帰属表示](https://www.openstreetmap.org/copyright)
`© OpenStreetMap contributors` を出す。タイルは OS のキャッシュディレクトリに
保存し（30 日で失効、200 MB を超えたら古いものから削除）、設定画面で場所と
使用量を確認でき、消去できる。

オフラインでもツールは動く。タイルを取得できないときは模式キャンバスに
「地図タイルを取得できません（オフライン）」と出るだけで、軌跡・再生・
GPX / KML / KMZ などの書き出しは止まらない。`--screenshot` と headless 描画は
タイルを取得しない（ネットワークを使わない）。外部（インターネット）へ要求を
出すのは GUI の地図タイルだけで、CLI の各コマンドはインターネットに接続しない。
NMEA 中継（`nmea proxy --bind`）は TCP で待ち受けるが、既定は loopback
（`127.0.0.1`）で、非ループバックを許すには `--allow-non-loopback` を明示する。

## インストール

### リリースバイナリ

`v1.0.0` のような安定版タグの push で GitHub Release が作られ、Ubuntu / Windows / macOS の
バイナリ、`LICENSE`、SHA-256 チェックサム（`SHA256SUMS.txt`）が添付される。**このリリース
手順はまだ一度も実行していない**（`v1.0.0` タグも Release も未作成）。初回のタグ push 後は、
リリースページ
（<https://github.com/haruki0420/holux-m241-rs/releases>）から自分の OS のファイルを取る。

| OS | CLI | GUI |
|---|---|---|
| Linux (x86_64) | `m241-linux-x86_64` | `m241-gui-linux-x86_64` |
| Windows (x86_64) | `m241-windows-x86_64.exe` | `m241-gui-windows-x86_64.exe` |
| macOS (aarch64) | `m241-macos-aarch64` | `m241-gui-macos-aarch64` |

取得したファイルは `SHA256SUMS.txt` と照合する（PowerShell では
`certutil -hashfile <ファイル> SHA256`）。アセット名は `m241-linux-x86_64` のように OS 名
つきなので、`m241`（Windows は `m241.exe`）など任意の名前にリネームしてよい。
インストーラは無く、`PATH` の通った場所に置けば動く（実行ビットが落ちている場合は
`chmod +x`）。Linux では udev（`libudev`）が、GUI では X11 / Wayland と OpenGL の
ライブラリが実行時に要る。端末にアクセスするユーザーは `dialout` グループに入れる
（`sudo usermod -aG dialout $USER` の後、ログインし直す）。

バイナリは**コード署名されていない**。macOS では Gatekeeper が検疫（quarantine）を付けて
起動を止めることがあり、その場合は「システム設定 > プライバシーとセキュリティ」で許可するか、
`xattr -d com.apple.quarantine m241` を実行してから起動する。Windows では SmartScreen が
「Windows によって PC が保護されました」と表示することがあり、「詳細情報 > 実行」で進む。
どちらも配布元（このリポジトリの Release）と SHA-256 を確認してから行う。SHA-256 は転送の
破損しか検出しない（署名・provenance は付けていない）。

Linux 版は Ubuntu 24.04（glibc 2.39）の runner でビルドしており、glibc 系の
ディストリビューションと `libudev.so.1` を前提とする（musl の静的バイナリではない）。
OS ごとの詳細は [`docs/user/01-install.md`](docs/user/01-install.md)（英語）にある。

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
# （1 回目の後、電池を抜き差しして Enter を押す）
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

日付修正を実際に行う手順は [`docs/user/02-fix-the-date.md`](docs/user/02-fix-the-date.md)
（英語）に、書き込みが失敗したときの復旧は
[`docs/user/03-recovery.md`](docs/user/03-recovery.md)（英語）にある。

## このリポジトリに含まれないもの

Holux のファームウェアイメージ、ダウンロードエージェント（`DA_v3.8.4.bin`）、ベンダーの DLL、
ベンダーの実行ファイルはプロプライエタリであり、このリポジトリでは再配布していない。これらの
ベンダーファイルは利用者が用意したローカルのフォルダから読み込む。

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
`check_layout` で検査する。読み取り専用の `m241-core` は書き込み・消去ルーチンを持たない。
実機での確認状況は [`docs/dev/05-verification.md`](docs/dev/05-verification.md) §1.5 を見る。

どこから何を取り入れたか、どの主張がまだ仮説かは
[`docs/dev/04-prior-art.md`](docs/dev/04-prior-art.md) に書いてある。

## 安全について

JTAG を持たない、生産終了から年数が経つ機器のファームウェアを書き換えるツールである。
書き込む前に [`DISCLAIMER.md`](DISCLAIMER.md) を読むこと。まず自分でファームウェアを吸い出して
保管し、書き込むのはそこから作ったイメージだけにする。

## ドキュメント

利用者向け:

| ページ | 何が分かるか |
|---|---|
| [`CHANGELOG.md`](CHANGELOG.md) | リリースごとの変更点と、既知の未検証項目 |
| [`docs/user/01-install.md`](docs/user/01-install.md) | OS ごとの導入、ポートの見つけ方、ベンダーファイルの用意（英語） |
| [`docs/user/02-fix-the-date.md`](docs/user/02-fix-the-date.md) | 日付を直す手順と、書き込み前の確認（英語） |
| [`docs/user/03-recovery.md`](docs/user/03-recovery.md) | 書き込みが失敗したときの復旧手順（英語） |
| [`DISCLAIMER.md`](DISCLAIMER.md) | 書き込みを行う前に読む免責事項 |
| [`PRODUCT.md`](PRODUCT.md) | 誰のためのもので、何をしてはいけないか |

開発者向け:

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
| [`AGENTS.md`](AGENTS.md) | 自動化されたコントリビューター向けの作業メモ |

## ライセンス

コードとドキュメントは MIT（[`LICENSE`](LICENSE)）。プロプライエタリなバイナリの権利は、
それぞれの所有者に帰属する。書き込みを行う前に [`DISCLAIMER.md`](DISCLAIMER.md) を読むこと。
