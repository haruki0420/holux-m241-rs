# ezTour 仕様化 — 振る舞いだけを取り出す（クリーンルーム）

このプロジェクトは Holux M-241 のログ読み出し・地図表示・GPX/KML 変換・写真
ジオタグを Rust で作り直す。その参照実装が **HOLUX ezTour for Logger v3.0**
（2017-09-28 ビルド）である。本ページはその**振る舞いの仕様**だけを書き出したもの。

- vendor のコード・文字列ダンプ・リソースは**このリポジトリに置かない**。本ページは
  観測結果を要約した文章であり、コピーではない。
- ezTour の著作権は消滅していない（Holux は deadpooled だが権利は残る）。移植はしない。
- 本ページの各節に「**どう確認したか**」を書く。推測は推測と明記する。
- 実装状況・未検証の台帳は [05-verification.md](05-verification.md) が唯一の置き場。
  本ページは「ezTour がこう動いていた」という事実の記録で、こちらの実装の主張ではない。

---

## 0. 参照した配布物と確認手段

配布物（`_scratch/inbox/holux_eztour_for_logger_v3.0_20170928.zip`、gitignore 対象、
コミット禁止）を `_scratch/eztour/unpacked/` に展開して調べた。展開は `7z x` で
そのまま通る（NSIS-2 の自己解凍 exe）。

| 構成物 | 実測 | 何に使ったか |
|---|---|---|
| `ezTour_DL.exe` | 19,288,064 B / 2017-09-28 / PE32 i386 / MFC | 本体。UI 文言・ダイアログ・埋め込み web 資産・DLL 呼び出し |
| `CoorCvt32.dll` | 53,248 B / 2011-07-18 / PE32 / MSVC8 | 座標変換。8 export |
| `HLX_GetTrkInfo.dll` | 14,336 B / 2010-11-12 / PE32 / MSVC8 | 軌跡の点ごとの情報計算。3 export |
| `exiftool.exe` | 2,933,448 B / 2009-08-11 / exiftool 7.17 | 写真への EXIF 書き込み（外部プロセス起動） |
| `users_manual.pdf` | 24 ページ / `ezTour_UM_EN_110927` / v2.0 | ユーザー操作フローの一次資料 |
| `libcef.dll` | 10,786,304 B / 2010 | 地図 UI は埋め込み Chromium（CEF）。ActiveX 版（`CWebBrowser2`）も同梱し、2 経路を持つ |
| `web/`（77 ファイル）、`icon/`（116 個の PNG） | | 地図・POI の見た目と JS 側の API 使用。展開物は合計 264 ファイル |
| `Holux_Driver/`（CP210x VCP など） | | M-241 は CP210x（VID 10C4 / PID EA60）の USB シリアル |

**確認手段**（すべて手元で再現できる）:

- DLL の export 表: PE の export directory を `llvm-readobj --coff-exports` で読む。
- export の**引数**はマングル名を `llvm-undname` に通して確定した。MSVC の
  `?Name@@YAHHHHPAU...@Z` は引数型と個数をそのまま持っているので、ここは推測ではない。
- 関数本体の意味: Ghidra 12.2 の headless（`analyzeHeadless` + 自作の
  全関数 decompile スクリプト）で C に落とし、`objdump -d -M intel` と突き合わせた。
  スクリプトとプロジェクトは `_scratch/` 側にだけ置き、コミットしない。
- 定数の意味: `.rdata` を Python で読み、`<d`（double）でデコードした。
- 呼び出し側: 本体の import 表（`objdump -p`）と、IAT スロットを狙う
  `ff 15 <abs>` パターンの全走査。
- 文字列: `strings -n 4` と `strings -el`（UTF-16LE）。**ダンプはコミットしない**。
  本ページには仕様として必要な語だけを書く。
- マニュアル: `pdftotext -layout`。

---

## 1. 全体の流れ（読み込み → 表示 → 変換）

**どう確認したか**: マニュアル全 24 ページの目次と本文、exe のコマンド名
（`gpsReadLog` / `gpsClearLog` / `gpsConfigGPS` / `gpsResetDevice` / `gpsReconnect` /
`exportTrack` / `exportKMZ` / `exportHtml` / `photoGeoTag` / `shiftPhotoTime` /
`addPhotoFromFolder` など）とダイアログ名（`CGPSConfigDlg`、`CTrackListDlg`、
`CMainAreaMapWnd`、`CMediaListDlg`、`CUploadLocrWizard` …）、同梱 web 資産の
コントロール用 JS。**起動時に自動で何かが走る形ではなく、すべてメニュー起点**。

1. **端末から読み出す**（`File > Read Device Log…`）
   - 初回のみプロダクトキー入力を求められる（§6）。
   - 読み方の選択: **「Read log only」**か**「Read log and import photo」**。
     後者は読み出しと同時に指定フォルダの写真を取り込む。
   - COM ポートとボーレートは**既定で自動スキャン**。手動指定も可。
     対応ボーレートの選択肢は 115200 / 57600 / 38400 / 19200 / 14400 / 9600 /
     4800 / 2400 / 1200。M-241 は 38400（既知の実測値）。
   - 読み出し後、**トラック一覧ダイアログ**が出て、取り込むトラックを選ぶ。
     選んだトラックが地図に描かれる。
   - 読み出し中の状態遷移は「読み出し → 消去 → 設定 → 再接続」のコマンド群で
     表現されている（`gpsReadLog` / `gpsClearLog` / `gpsConfigGPS` /
     `gpsResetDevice` / `gpsReconnect`）。
2. **端末側の設定**（`File > Config GPS…`）
   - 接続設定（自動/手動）と **GPS Log Setting**。
   - ログモードは 4 種類: **General / Vehicle / Cycling / Hiking**。
   - 記録間隔: `Log every ( ) seconds` と `Log every ( ) meters` の両方を持つ。
   - メモリ残量バーと `Clear Log`。
   - メモリ満杯時の挙動: **Overwrite**（古い順に上書き）か **Stop Log**（停止）。
3. **一覧と表示**
   - トラック一覧で**色と線幅**を変更できる。`Play` で地図上を現在位置が
     移動しながら再生（`Photo Slide Show` を有効にすると写真も同期）。
   - POI: 端末の POI ボタンで打った地点が軌跡上にアイコンで出る。クリックで
     名前とメモを編集。手動追加（青いバルーン → 地図クリック → プロパティ）。
   - 写真は取り込むと**撮影時刻と軌跡から自動マッチング**され、
     プレースマーク（写真の入れ物）が作られる。前/次ボタンで写真と現在位置を同期。
4. **編集**
   - トラック編集: waypoint の Shift+クリック範囲選択、Ctrl+クリックのトグル、
     矩形エリア選択、削除、**Split Track**（右クリック）、**Merge Tracks**、
     複数段の Undo/Redo。
   - `Shift photo time…` で全写真の撮影時刻をまとめてずらし、**ずらした後に
     再マッチング**する。
5. **保存・書き出し**
   - プロジェクト保存（`.itm`）。写真・写真コメントも復元される。
   - GPX / NMEA / KML / CSV への書き出し、KMZ 書き出し、HTML（`.htm`/`.mht`）
     書き出し、Google Earth で表示、トラック/ルートのレポート（CSV）。
6. **外部サービス**（本プロジェクトでは**対象外**、§6 で明記）
   - Flickr / Locr / Facebook へのアップロード。

**この流れの要点**: ezTour は「端末 → 内部モデル（トラック + POI + 写真 +
プロジェクト）→ 表示（地図）→ 書き出し」の 4 段で、各段の間に独自形式は
**プロジェクトファイル（`.itm`）だけ**。`.itm` は SQLite 3 のデータベースで
（`SQLite format 3` と `CREATE TABLE`/`INSERT INTO` の書式文字列、
`IT_TGVLib` というスキーマ名の接頭辞が残っている）、写真・コメント・
プロジェクト名をテーブルで持つ。**仕様として模倣する必要はない**が、
「プロジェクト = 単一ファイル + 参照写真」という粒度は参考になる。

---

## 2. 対応フォーマット（読み / 書き）

**どう確認したか**: exe のファイルダイアログのフィルタ文字列、書き出しテンプレート
（GPX/KML の要素名）、一覧の並び。「どの形式を読めるか」はダイアログのフィルタと
`Read log from files` の一覧（`Dir` / `File Name` の 2 列）から。

| 形式 | 読み | 書き | 備考（確認できた範囲） |
|---|---|---|---|
| 端末のバイナリログ | ○ | — | シリアル経由。拡張子は `.bin` 系のダンプも読み込める（`%s*.bin`） |
| NMEA 0183（`.nmea` / `.NMA`） | ○ | ○ | `$GPRMC` / `$GPGGA` / `$GPGSA` を組み立てる書式が本体にある。書き出しは NMEA 0183 テキスト |
| GPX | ○ | ○ | **1.0 と 1.1 の両対応**。書き出しは 1.1 が既定（`trk`/`trkseg`/`trkpt`、`wpt`、`rte`/`rtept`） |
| KML | ○ | ○ | KML 2.0（`Document`/`Folder`/`Placemark`/`Point`/`LineString`/`Style`） |
| KMZ | ○ | ○ | KML + 写真 + アイコンを zip。一時ディレクトリ経由で組み立てる |
| CSV | — | ○ | トラック点とレポートの 2 種類。列は INDEX / TRACK NUMBER / UTC DATE / UTC TIME / LOCAL DATE / LOCAL TIME / LATITUDE / N/S / LONGITUDE / E/W / ALTITUDE / SPEED |
| `.itm` | ○ | ○ | プロジェクト（SQLite 3）。単一ファイル |
| `.itx` | ○ | ○ | 旧「GPS Photo Tagger」のプロジェクト。読み込み互換 |
| `.dst` | ○ | — | 「Destination File」。目的地リスト。`preload.dst` を新規プロジェクトに先読みする設定がある |
| `.tki` | ○ | ○ | トラック情報のキャッシュ（`%s%s.tki`） |
| HTML / MHT | — | ○ | 地図の静的スナップショット（`beforeFrame_%d.html` 等のフレーム書き出し） |
| RTF | — | ○ | レポート印刷 |
| Yupiteru 形式（`$YPLOG` / `$YPDRL`） | ○ | — | 他社ロガーのログ取り込み。GPX の creator が YUPITERU になる経路がある |

**GPX 書き出しの中身**（テンプレートから読める範囲）:

- ヘッダは GPX 1.1、`creator` は `HOLUX ezTour Plus`。`metadata` に
  `link`（holux.com）と `time`（UTC, `%Y-%m-%dT%H:%M:%SZ` 相当）。
- `trk` → `trkseg` → `trkpt lat="%.9f" lon="%.9f"`。各点に `ele`、`time`、
  `speed`、`name`/`desc` の CDATA 版と素の版の両方が用意されている。
- `extensions` に `extType` を書ける。機種依存の拡張（速度・温度・心拍・ケイデンス:
  `speed` / `temp` / `bpm` / `rpm`）がある。
- 「**GPX を簡略化するか**」を聞くチェックがある（Web アップロード向けの点間引き）。

**KML 書き出しの中身**:

- `kml 2.0`、`Document` → `Folder` → `Placemark`。
- 線は `LineString`（`tessellate` + `coordinates`）、点は `Point`。
- スタイルは `Style` / `IconStyle`（`scale`/`Icon`/`href`）と `LineStyle`
  （`color`/`width`）。「線の色と太さ」「アイコンの URL」がそのまま KML に出る。
- `LookAt` に `longitude`/`latitude`/`range`/`tilt`/`heading`。
- 座標は `%f,%f,%f`（lon,lat,alt の順、KML の約束どおり）。

---

## 3. `CoorCvt32.dll` — 8 export の意味

**どう確認したか**: export 表（8 件、ordinal 1–8）を `llvm-readobj --coff-exports`
で取得。マングル名を `llvm-undname` で戻して**引数の型と個数を確定**（ここは
推測ではなく署名そのもの）。本体は Ghidra headless で decompile し、`.rdata` の
定数を Python で double としてデコードして突き合わせた。呼び出し側は本体の
import 表と IAT 走査。

### 3.1 8 export の一覧（署名は確定、挙動は本文から）

| # | export | 署名（demangle 結果） | 役割（本文から読めたこと） |
|---|---|---|---|
| 1 | `Coord_id` | `int Coord_id`（**変数**） | 直近に使われた**座標形式コード**を保持する公開グローバル。`.data` 初期値 0 |
| 2 | `Cvt` | `int Cvt(int coord, int datum, int, coordinate* in, coordinate* out, wchar_t* s1, wchar_t* s2, datum_trans* custom, TM* tm, unsigned)` | **緯度経度 → 指定形式**。数値 3 つ（lat, lon, alt）と、表示用のワイド文字列 2 本を返す |
| 3 | `CvtRvs2` | `int CvtRvs2(int coord, int datum, int, wchar_t* s1, wchar_t* s2, coordinate* out, wchar_t* o1, wchar_t* o2, datum_trans*, TM*)` | **文字列 → 緯度経度**（`Cvt` の逆入力版）。入力は 2 本のワイド文字列 |
| 4 | `CvtRvs2WGS84` | 同上（10 引数） | `CvtRvs2` の結果を、そのまま **WGS84 の緯度経度**へ寄せる薄いラッパー |
| 5 | `CvtRvs` | `int CvtRvs(int datum, int, coordinate* in, coordinate* out, wchar_t* s1, wchar_t* s2, datum_trans*, unsigned)` | **指定形式 → 緯度経度**（数値入力版）。データムの逆変換と整形 |
| 6 | `Datum_id` | `int Datum_id`（**変数**） | 直近に使われた**データム ID**。`.data` 初期値 2 |
| 7 | `UTM2LL` | `int UTM2LL(int zone, wchar_t* in1, wchar_t* in2, coordinate* out, int, wchar_t* o1, wchar_t* o2, unsigned)` | **UTM 文字列 → 緯度経度**。`zone` は 0/1 の範囲チェックあり |
| 8 | `toTM2` | `void toTM2(double,double,double,double,double,double,double, double* x, double* y)` | **横メルカトル正投影**の汎用関数（7 個の double と出力 2 本） |

`Coord_id` は実体が `0x1000A38C`、`Datum_id` は `0x1000A010` で、どちらも
他の関数から読み書きされる**状態変数**。ただし `Cvt` は引数の `coord` /
`datum` をそれぞれの変数に**書き込むだけ**で、変換には引数の値を使う。
ezTour は毎回 `push [ebp+0x8]`（coord）と `push [ebp+0xc]`（datum）で
自分の値を渡しているので、**これら 2 つの初期値は「ezTour の既定表示」を
意味しない**。

**重要**: ezTour 本体が import しているのは **`Cvt` の 1 個だけ**。`CvtRvs` /
`CvtRvs2` / `CvtRvs2WGS84` / `UTM2LL` / `toTM2` は ezTour からは呼ばれていない
（import 表に無い）。他社/他製品向けの汎用 DLL として export されている。
また `toTM2` は DLL 内部からの直接呼び出しも無い（`call 0x10003b00` が 0 件）。
つまり **ezTour が実際に使う経路は `Cvt` 1 本**で、残りは「DLL の公開 API」。
再実装では `Cvt` 相当だけ作れば足りる。

### 3.2 構造体

本文のコピー幅から決まる（推測ではなく、`mov` の長さの観測）。

- `coordinate` = **24 バイト = double 3 個**。`Cvt` は `+0` と `+8` を緯度・経度、
  `+0x10` を高度として読み、`CvtRvs2` は同じ 24 バイトをゼロクリアする。
  `Cvt` の出力 `coordinate` には変換後の緯度・経度・高度が入る。
- `datum_trans`（ユーザー定義データム） = **double 2 個**。`+0` に長半径 `a`、
  `+8` に**逆扁平率** `1/f`（データム表のレコードと同じ並び）。
- `TM`（ユーザー定義投影） = **少なくとも double 5 個**。`+0x20` は 0.5、
  `+0x10`/`+0x18` は 100.0、`+0`/`+8` は 0.0 と 89.0 と比較され、
  差が 1.19e-7 未満なら「標準値」とみなす判定に使われている
  （`+0`/`+8` は真偽のフラグ的な比較で、投影値そのものではないと読める）。
  中央子午線・原点緯度・縮尺・偽東距・偽北距に相当する 5 値と読めるが、
  **各フィールドの役割の割り当ては推測**。
- `Cvt` の 10 番目の引数（`unsigned`）は本文で**一切読まれていない**
  （`[ebp+0x2c]` の参照が 0 件）。呼び出し側は **0x3A8（= 936）** を渡す。
  互換のための予約と見られる（推測）。

### 3.3 座標形式コード（`Coord_id`、0〜13 の 14 種）

**どう確認したか**: `Cvt` の本体が `param_1`（coord）で分岐する `if` の連鎖を
逆アセンブルで追い、**各分岐が呼ぶヘルパー関数**を特定した。そのヘルパーが
参照する `.rdata` の double 定数を全部デコードし、**公開されている各国グリッドの
定義値と照合**した。書式（`%c%c %c %d` など）は分岐ごとの `sprintf` から。
上限は `CvtRvs2` の入口で `0xd < param_1 → エラー` として確定（0〜13）。
UI 側の名前は exe の文字列リソースから取り、**並び順と定数の両方**で対応を決めた。

| code | UI 上の名前 | 出力 | 根拠（分岐先の定数） |
|---|---|---|---|
| 0 | Lat/Lon | **度分秒**（または 10 進度） | DMS 整形関数を呼ぶ |
| 1 | UTM | ゾーン + バンド文字 + 整数 m | `0.9996`・偽東距 `500000`・南半球 `+10,000,000` |
| 2 | MGRS | ゾーン + 100km 区画 2 文字 + 数字 | `I`/`O` を除く 24 文字 `ABCDEFGHJKLMNPQRSTUVWXYZ` と 100km / 2,000,000 / 100,000 の剰余 |
| 3 | RSO | **斜軸投影の東距・北距（小数 3 桁）** | 偽東距 **804 671.2996 m** と回転を含む定数群（`0.6`/`0.8`/`1.00330320918`/`2.99133e-06`）。RSO（マレー半島）の定義値と一致（**国名の同定は推定**） |
| 4 | Maidenhead Locator | 英数字 8 文字 | `A`–`R` と `0`–`9` の 4 段、基数 `×20`/`×10`/`×5`/`×2` |
| 5 | Austria Grid | 整数 m | Bessel 1841 + 偽東距 `150000`/`450000`/`750000` の 3 帯、経度 `8.833`–`17.833°` で切替 |
| 6 | British Grid | 整数 m | 縮尺 **`0.9996012717`**（OSGB36 の定義値）と子午線弧の級数項（`61`/`58`/`18`/`5040`/`14`/`479`/`179`） |
| 7 | Finnish Grid | 整数 m | International 1924（a=6378388, 1/f=297）+ 偽東距 `500000` + 中央子午線 `21°/24°/27°` + `1000000 × 帯番号` のノルウェー式ノーシング。**KKJ の定義値と整合**（帯の算術そのものは完全には追えていない） |
| 8 | German Grid | 整数 m | Bessel 1841 + Gauss-Krüger の偽東距 `2500000`/`3500000`/`4500000`/`5500000`、経度 `4.5`–`16.5°` の 3° 帯 |
| 9 | HK80 | 整数 m | 原点緯度 `22.312133`・中央子午線 `114.178556`・縮尺 `1.0`・偽東距 `836694.05`・偽北距 `819069.8` |
| 10 | Swenden Grid（＝Sweden） | 整数 m | Bessel 1841 + 中央子午線 **`15.808278°`**・縮尺 `1.0`・偽東距 `1500000`（RT90 の定義値） |
| 11 | Swiss Grid | 整数 m | Bessel の `e²=0.006674372`・偽東距 **`600000`**・偽北距 **`200000`** の斜軸メルカトル（CH1903 の定義値） |
| 12 | Taiwan Grid | 整数 m | 中央子午線 `121.0`・縮尺 `0.9999`・偽東距 `250000`・偽北距 `0`、経度 `120.0`–`122.0°`。**TWD97 の TM2 121°E 帯の定義値そのもの** |
| 13 | User Defined | 整数 m | 引数の `TM*` が NULL なら即エラー。5 つの投影パラメータで計算 |

**コードと名前の対応**: UI の一覧は 13 個で、並びは **`Taiwan Grid` → `Swiss Grid` →
`Swenden Grid` → `HK80` → `German Grid` → `Finnish Grid` → `British Grid` →
`Austria Grid` → `Maidenhead Locator` → `RSO` → `MGRS` → `UTM` → `Lat/Lon`**。
これは **code 12 → 0 の降順**に一致する。値から名前を独立に同定できたのは
Taiwan(12) / Swiss(11) / Sweden(10) / HK80(9) / German(8) / British(6) /
Austria(5) / Maidenhead(4) / RSO(3) / MGRS(2) / UTM(1) / Lat/Lon(0) の 12 個で、
すべて逆順の対応と一致した（残る Finnish(7) だけは定数からの同定が弱い）。

投影の実体は「**その国・その測地系の固定パラメータで横メルカトル
（または斜軸）に投影し、東距・北距を整数 m で出す**」という同じ形。
`FUN_10001e00` の引数順は本体コードから確定していて、
`(出力E, 出力N, 長半径 a, 逆扁平率 1/f, 原点緯度, 中央子午線, 縮尺 k0, 偽東距, 偽北距, 緯度, 経度)`
である（UTM の分岐が `0.0, zone*6-183, 0.9996, 500000, 0.0` を渡すことから確定）。
再実装で真似する価値があるのは **UTM(1) と 台湾 TM2(12) の 2 つ**だけで、
残りは特定の国の測地系に依存する過剰装備。日本の測地系（Tokyo）向けの
グリッドは一覧に無い。

### 3.4 データム（`Datum_id`）

**どう確認したか**: `Cvt` の分岐 `if (Datum_id == 0xdf)` / `if (Datum_id == 0xe4)`、
上限チェック `0xe4 < param_2 → エラー`、`.rdata` のテーブル（先頭 0x100072A8、
**32 バイト×228 レコード**）を Python でデコードした結果。

- テーブルは **228 件（ID 0〜227）**。1 レコード = 32 バイト:
  `+0` = 楕円体パラメータ（`double a`, `double 1/f`）へのポインタ、
  `+8`・`+16`・`+24` = **`dx`, `dy`, `dz`（メートル）**。
  回転・縮尺の項は無い → **3 パラメータの平行移動**だけを行う実装。
- ID **0xDF（=223）** のときは**データム変換（3 パラメータの平行移動）を
  スキップ**する分岐がある（`cmp Datum_id, 0xDF` で飛ぶ）。テーブルの 223 は
  たまたま `WGS 1972`（shift 0,0,0）なので、数値上は「移動ゼロ」と同じ結果になる。
- ID **0xE4（=228）** は**テーブルの範囲外**（0〜227 の次）で、
  **ユーザー定義データム**のスロット。このときは `datum_trans*` が必須で、
  `a` と `1/f` をそこから読む。UI の「User Defined…」がこれに当たる。
- 楕円体は 25 種類が使われている。主なもの: WGS84（a=6378137, 1/f=298.257223563）、
  GRS80 相当（1/f=298.257222101）、International 1924（a=6378388, 1/f=297）、
  Clarke 1880（a=6378249.145, 1/f=293.465）、Clarke 1866（a=6378206.4, 1/f=294.9786982）、
  Bessel 1841（a=6377397.155, 1/f=299.1528128）、Airy（a=6377563.396, 1/f=299.3249646）、
  Krassovsky（a=6378245, 1/f=298.3）、Australian National（a=6378160, 1/f=298.25）。
- UI のデータム名は **228 個**（+「User Defined…」）。TWD'97 / TWD'67 /
  Beijing 1954 / xi an 80 / Hu-Tzu-Shan / Tokyo-Japan / Tokyo-Okinawa /
  Tokyo-South Korea / MGI / WGS 1972 / WGS 1984 など、台湾・中国・日本の
  測地系が含まれる。名前は**ほぼアルファベット降順**（`AZanderij` で始まり
  `Adindan-Burkina` で終わる。地域別の派生名が親の直後に来るため、厳密な
  ソートにはなっていない）。
- **ID と名前の対応規則は確定した**: テーブル ID `n` の名前は
  名前一覧の **`227 − n`** 番目（＝テーブルは名前の**昇順**、名前一覧は**降順**）。
  公開されている各測地系の定義値と **18 件すべて一致**した:
  - `WGS 1984` → ID 224。a=6378137, 1/f=298.257223563, shift (0,0,0)。WGS84 そのもの。
  - `WGS 1972` → ID 223。a=6378135, 1/f=298.26, shift (0,0,0)。
  - `TWD'97` → ID 218。a=6378137, 1/f=298.257222101（GRS80）, shift (0,0,0)。
  - `TWD'67` → ID 217。a=6378160, 1/f=298.25（GRS67）, shift (−685, −470, −237)。
  - `Adindan-Burkina` … `Adindan-Sudan` → ID 0–6 に 1 対 1。shift も
    (−118,−14,218) 〜 (−161,−14,205) まで既知の Adindan 系の値と全件一致。
  - `Afgooye-Somalia` → ID 7。Krassovsky（a=6378245, 1/f=298.3）, (−43,−163,45)。
  - `Ain el Abd'70-Bahrain` → ID 8、`-Saudi Arabia` → ID 9（International 1924）。
  - `American Samoa'62` → ID 10。Clarke 1866, (−115, 118, 426)。
  - `Hu-Tzu-Shan` → ID 83。International 1924, (−637, −549, −203)。
  - `AZanderij` → ID 227、`Yacare` → ID 226（一覧の先頭 2 件が末尾 2 件に来る）。
- **日本のデータムも入っている**: `Tokyo-Japan` / `Tokyo-Okinawa` /
  `Tokyo-South Korea` の 3 件（日本測地系の地域別 3 パラメータ）と `MGI`。
  日本で使うならこのいずれかを選ぶ形になる。
- **エクスポート変数の初期値**: `Datum_id`（ordinal 6、`?Datum_id@@3HA`）の
  `.data` 初期値は **2**、`Coord_id`（ordinal 1）は **0**。ただしこれは
  DLL を読み込んだ直後の値で、**ezTour は呼び出しのたびに自分が保持する
  座標形式とデータムを引数で渡している**（呼び出し直前の `push [ebp+0x8]` /
  `push [ebp+0xc]`）。したがって「ezTour の既定表示が何か」は DLL からは
  決まらない。**アプリ側の既定値は未確定**（レジストリの
  `OptionsDatum` / `Datum1` / `Datum2` 系のキーに保存されているところまで確認）。

### 3.5 `toTM2` の引数（7 double）

**どう確認したか**: decompile 本文の使われ方。`e² = 2/(1/f) − 1/(1/f)²` の形から
第 2 引数が**逆扁平率**、`× 0.01745329251994`（度→ラジアン）が掛かる位置から
第 3・第 5・第 6 引数が**度で表した角度**、最後に両出力へ掛かる係数から
第 7 引数が**縮尺 `k0`**、第 1 引数が**長半径 `a`**、と読める。

```
toTM2(a, 1/f, <deg>, <deg>, <deg>, <deg>, k0, double* east, double* north)
```

第 4 引数の使われ方は本文から特定できなかった（出力計算に現れない）。
**引数の並びのうち 4 番目だけは推測**。子午線弧長の計算、`e²` のべき級数、
UTM と同じ `k0`/偽東距の形を持ち、**2 度分帯でも 6 度分帯でも同じ関数で
中央子午線を変えるだけで済む**作りになっている。これは台湾の TM2
（121°E / 119°E の 2 度帯）を扱うための構造と整合する。

### 3.6 数値の丸め・表示規則

DLL に残っている `sprintf` 書式はこの 6 種（他は整数化だけ）。
数値の丸め位置は書式がそのまま仕様になる。

| 書式 | 使いどころ |
|---|---|
| `%c %3d^%02d'%06.3f"` | **度分秒**（度 + 分 2 桁 + 秒 6 桁・小数 3 桁）。半球は別の `%c` |
| `%c %3d^%08.5f'` | 度 + **分だけ小数 5 桁**の別表記（分までを小数で出す形） |
| `%c %12.8f^` | **10 進度・小数 8 桁**（度記号つき）。DMS の代わりに使う経路 |
| `%6.3fE` / `%6.3fN` | 東距・北距を**小数 3 桁**で（RSO = code 3） |
| `%7dE` / `%7dN` | 東距・北距を**整数 m**で（UTM・各国グリッド） |
| `%s %7dE` / `%s %6dE` | グリッド名/区画の接頭辞 + 整数 m |

度記号は出力時に `^` を `°`（U+00B0）へ置換して作る（ワイド文字列の
1 文字置換）。**負の緯度経度は半球文字（N/S/E/W）で表す**（符号では出さない）。
「`^` を使っている」ところから、元は ASCII だけで組んで後から記号に直す
設計だったと読める。

---

## 4. `HLX_GetTrkInfo.dll` — 3 export

**どう確認したか**: export 表（3 件）と demangle、Ghidra での decompile、本体の
import 表 + IAT 走査で**呼び出し箇所と引数**を確認した。3 つとも ezTour から
import・呼び出しされている（`CoorCvt32` と違ってこちらは実際に使われている）。

| export | 署名 | 確認できた挙動 |
|---|---|---|
| `HLX_TrackInit` | `int HLX_TrackInit(int, int)` | 内部状態を初期化する。**0x140 バイトを `new`** してゼロ埋めし、引数を 2 つのグローバル（16bit 用と 4bit 用）に保存。既に確保済みなら**先に delete してから作り直す**。確保失敗で 0 |
| `HLX_TrackAddPoint` | `int HLX_TrackAddPoint(HLX_POINT*, HLX_TRKINFO*)` | 点を 1 つ追加し、**軌跡情報を計算して `HLX_TRKINFO` に書く**。どちらかが NULL なら 0、成功で 1 |
| `HLX_TrackExit` | `void HLX_TrackExit(void)` | 内部状態を解放し、グローバルをゼロに戻す。二重呼び出しに耐える（NULL チェックあり） |

本体側の呼び出しは `HLX_TrackInit(4, 650)`（実測。cdecl の push 順から
第 1 引数 = 4、第 2 引数 = 650）。650 はレコード内の 16bit フィールドに、
4 は 4bit のフラグに使われる。

### 4.1 `HLX_TRACKAddPoint` がやっていること

- **内部の packed レコード（36 バイト = 9 dword）を組み立てる。**
  入力 `HLX_POINT` から読むのは、`WORD +0x00`、`BYTE +0x02, +0x06, +0x08,
  +0x0a, +0x0c, +0x20, +0x24, +0x26`、`DWORD +0x10, +0x14, +0x18, +0x1c`
  （逆アセンブルの読み出し幅そのもの）。これをビットフィールドに詰め替えて
  内部状態の `+0x24` にコピーする。**packed レコードが端末のバイナリログと
  同型**（日時・緯度・経度・高度などをビットで詰める）に見えるが、
  **端末側の実フォーマットとの同一性は未確認**（推測）。
- **日時は 31bit の packed 値**として扱う。デコード規則は本文から確定:
  `年 = ((v >> 26) & 0x1F) + 2000`、`月 = (v >> 22) & 0xF`、`日 = (v >> 17) & 0x1F`、
  `時 = (v >> 12) & 0x1F`、`分 = (v >> 6) & 0x3F`、`秒 = v & 0x3F`
  （5+4+5+5+6+6 = 31 bit、年は 2000 年起点）。
- **`HLX_TRKINFO` に計算結果を書く。** 書かれる欄は少なくとも:
  開始/終了/現在の日時（packed のデコード結果を 4 組）、移動距離、速度、
  平均速度、最高速度、経過時間、高度、および内部の統計行列。
- **速度・距離の計算**は別関数群で、`時刻差（秒）` を 86400 で巻き戻す
  ラップ処理（日跨ぎ）と、速度域ごとの係数テーブル（低速度ほど大きい係数）を
  持つ。**どの速度域でどの係数か、の全表は取得していない**（一部のみ）。
- 距離の積算は「前点との差分」を逐次足す形で、内部に共分散行列に相当する
  7 つの統計量（Σdx, Σdy, Σdx², Σdy², Σdxdy, Σdx·alt, Σalt·dy）を持つ。
  これは**Kalman フィルタ的な平滑化の痕跡**だが、フィルタ本体の式は特定していない
  （推測: 位置の平滑化と速度推定に使っている）。
- 最後に「距離・時間から求めた平均速度」を `HLX_TRKINFO` の別欄へ書く。

### 4.2 再実装への示唆

ezTour の「速度・距離・標高の表示」は端末から来た生の値ではなく、
**点列から再計算した値**を使っている。M-241 のログに速度フィールドが
無い/粗い場合でも、ezTour は自前で速度を出していたことになる。
再実装で「端末の生速度」と「再計算速度」のどちらを出すかは**決める必要がある**
（→ [06-open-questions.md](06-open-questions.md) に追記候補）。

---

## 5. 地図表示の要件

**どう確認したか**: 同梱の `web/js/gmap/`（Google Maps **v2** 版）と
`web/js/gmapV3/`（**v3** 版）の JS を UTF-16LE から UTF-8 に変換して読み、
exe 側の地図初期化テンプレート（JSON 文字列群）と突き合わせた。地図は
**`libcef.dll` の埋め込みブラウザ**で、アプリが一時 HTML を生成して読み込ませ、
JS → アプリはコマンド文字列（`onTracker:lat:lon` など）で戻す。

- **地図 API**: Google Maps JavaScript API。v2（`GMap2` / `GLatLng`）と
  v3（`google.maps.Map` / `LatLng` / `Polyline` / `Marker` / `InfoWindow` /
  `MarkerImage` / `Size` / `Point` / `MVCArray` / `LatLngBounds`）の**2 実装を
  同梱**し、起動時にどちらを使うか選ぶ。API キーは**ソースに埋め込み固定**
  （値はここに書かない）。**どちらも現在は動作しない**（§6）。
- **地図タイプ**: `ROADMAP` / `SATELLITE` / `HYBRID` / `TERRAIN` の 4 種。
- **軌跡の描画**: 1 トラック = 1 ポリライン。**既定色は 10 色パレットの巡回**
  （紫→ピンク→オリーブ→…の 10 色）、**既定の線幅 5 px、不透明度 0.7**。
  トラックごとに色と幅を上書きできる。色は `#rrggbb` 文字列で JSON に載る。
- **ズームと視界**: 全トラックの外接矩形（min/max lat/lon）を JSON で受け取り
  `fitBounds`。現在位置への追従は `panTo`。再生（`onTracker`）では
  **ズームが 12 未満なら 12 に上げる**。ミニマップは現在ズーム +3（18 超なら −3）。
- **マーカー**: POI は種別ごとのアイコン（同梱 `icon/` に **116 個の PNG**:
  飲食・宿泊・空港・教会など）。トラックの始点は `line_start`、終点は `line_end`、
  現在位置は車の向き別 GIF（`web/js/imgctrl/images/` に `car000`〜`car315` の
  **45° 刻み 8 方向** + 静止用 `car.gif`）。写真は `camera`、目的地は `destination`。
  画像ファイルは展開物全体で 172 個（`icon/` 116 + `web/` 55 + インストーラ 1）。
- **再生（Play）**: 現在位置マーカーを点列に沿って動かす。写真スライドショー
  を同時に回せる。地図側の `onPhotoSlideShow` と `onPhotoShow` が写真を出す。
- **情報ウィンドウ**: マーカークリックで POI 情報（名前・コメント・写真数）。
  トラックの始点マーカーは名前と距離。
- **HTML 書き出し**: 同じ JS と CSS を**インライン展開**して 1 ファイルにする
  （`<!-- embedded CSS file from ... -->` のような目印つき）。地図の
  スナップショットを複数フレーム（`beforeFrame_%d.html`）で書き出す経路もある。
- 取得できない/失敗したときは **「Google Maps の代替ページ」**を出し、
  ActiveX ブロック時の対処文まで同梱している（＝当時から壊れやすかった）。

**再実装での読み替え**: 必要なのは「10 色パレット・幅 5・不透明度 0.7 の
ポリライン」「全点 fit」「追従時 zoom ≥ 12」「POI 種別アイコン」
「始点/終点/現在位置の区別」「再生時の点移動」「写真同期」の 6 点。
API は自前のタイル地図（またはローカル地図）に置き換える前提で、
**上記の数値と振る舞いだけ**を引き継ぐ。

---

## 6. 写真ジオタグの仕様

**どう確認したか**: マニュアルの GeoTag / Shift Photo Time / Options の章、
exe のオプション設定キー（`photoTimeZone` / `maxPhotoDiffTime` /
`groupPhotoDistance` / `kmzMaxPhotoEdgeSize` / `DaylightSaving` /
`PhotoDaylightSaving`）、同梱 `exiftool.exe`（7.17）の**起動引数の書式文字列**。

### 6.1 マッチングの規則

1. 写真の撮影時刻（EXIF の `DateTimeOriginal`）を読む。
2. **「写真のタイムゾーン」**（カメラ側の設定。既定は PC のタイムゾーン）で
   UTC に直す。夏時間は別チェックボックス。
3. 軌跡の各点の時刻と比較し、**最も近い点**を探す。
4. その差が **「Max photo/track diff time」（分）**を超えていたら**マッチさせない**
   （写真はトラックに紐づかず、位置も付かない）。
5. マッチした写真は**プレースマーク**に置かれる。**2 枚の写真の距離が
   「Group photos when distances is less than」（メートル）未満なら同じ
   プレースマークにまとめる**（＝建物の中などで撮った写真を 1 点に集約）。
6. 「表示用タイムゾーン」は**表示だけ**に効く（撮影側とは別設定）。
7. `Shift photo time…` で全写真の時刻をまとめてずらせる（日/時/分/秒）。
   **ずらした後は自動で再マッチング**する。カメラ別（`byCamera`）/
   選択のみ（`selPhotos`）/ 全部（`allPhotos`）の 3 つの適用範囲がある。
8. GPS 情報の無い写真も取り込める（位置はプレースマーク経由で付く）。

### 6.2 書き込む EXIF

GeoTag は**自前実装ではなく同梱 exiftool を外部プロセスで起動**し、1 回の
コマンドライン引数で GPS IFD を書く。exe に残っている書式文字列は次の 2 本
（値の並びだけ示す。実際の写真パス等は当然ながらここに書かない）:

```
-GPSVersionID=<v> -GPSLatitudeRef=<c> -GPSLatitude=<deg>
-GPSLongitudeRef=<c> -GPSLongitude=<deg>
-GPSAltitudeRef=<s> -GPSAltitude=<m> <file>
-TagsFromFile "<src>" -all:all "<dst>"
```

| タグ | 書き方 |
|---|---|
| `GPSVersionID` | 引数は **`0.0.2.2`** という固定の並びで渡している。EXIF 規格での並びは 2.2.0.0 なので**桁順が規格と逆**だが、実機で書かせて確認していないので**意図的なのか不具合なのかは不明**（推測: 当時の exiftool 7.x の受け付け方に合わせた表記） |
| `GPSLatitudeRef` / `GPSLongitudeRef` | 半球を**文字列**で渡す（`N`/`S`、`E`/`W` 相当） |
| `GPSLatitude` / `GPSLongitude` | **10 進度を `%f`（6 桁）で渡す**。度分秒への変換は exiftool 側に任せる |
| `GPSAltitudeRef` | 基準を**文字列**で渡す（海抜 / 海面下）。数値 0/1 ではない |
| `GPSAltitude` | メートルを `%f` で渡す |

- 既存 EXIF を壊さないよう、**`-TagsFromFile <元> -all:all <先>` で全タグを
  引き継ぐ**手順を別コマンドとして持つ（＝いったん別名に退避して書き戻す）。
- 書き込み前に **`_original` 付きのバックアップ名**を残す（exiftool の既定動作）。
- `-DateTimeOriginal="%.4d:%.2d:%.2d %.2d:%.2d:%.2d"` で**撮影時刻そのものを
  書き換える**経路がある（`Shift photo time` の実体）。
- 拡張子で対象を判定（JPEG / TIFF）。それ以外（MP4 等のメディア）は
  **ジオタグ対象外**だが、トラックへの紐づけはできる。

### 6.3 KMZ に同梱する写真

- 写真は KMZ 内の `photo/` に、サムネイルは `icon/`（または縮小版）に入る。
- **「Photo size in KMZ」（`kmzMaxPhotoEdgeSize`、ピクセル）**で長辺を縮小する。
- 一時ディレクトリ（`C:\Temp\GoogleEarth` 配下）で `track.kml` + 写真を組み立てて
  zip 化し、最後に一時ファイルを片付ける。

**再実装での読み替え**: EXIF 書き込みを外部 exe に依存しない（自前で
APP1/GPS IFD を書く）方針は正しい。仕様として引き継ぐのは
**「GPS IFD の 5 タグ + バックアップ + 既存タグ保持」**、マッチングは
**「最近傍点 + 上限時間 + 距離によるグループ化 + タイムゾーン変換」**の 4 点。

---

## 7. 死んでいる依存（再実装で持たないもの）

**どう確認したか**: exe 内の URL 文字列、同梱 JS の API 呼び出し、マニュアルの
記述。**サービス側の停止時期は外部知識**（この配布物からは分からない）ので、
「現在は動かない」は外部分野の判断として書く。

| 依存 | 実体 | 状態 |
|---|---|---|
| **Google Maps JavaScript API v2** | `maps.google.com/maps?file=api&v=2.250&key=…` を同梱 JS から読み込む | **停止済み**（v2 は 2013 年に新規提供終了、2016 年に完全停止）。埋め込みキーも無効 |
| **Google Maps JavaScript API v3（旧キー方式）** | `maps.google.com/maps/api/js?v=3&sensor=false&key=…` | キーが無効。現行の v3 は課金 + 新キー必須で、この実装のままでは動かない |
| **プロダクトキー認証サーバ** | `www.holux.com/eztour/sn/register.jsp?...`（email / software / version / device / timezone を渡す） | **取得不可**。holux.com のサービスは終了（deadpooled） |
| **一次ライセンス認証（iTravel-Tech）** | `www.iTravel-Tech.com:3000/prod/activate` と `/prod/validate` に multipart POST | **取得不可**。`:3000` のサービスは存在しない |
| 30 日トライアル管理 | 自前の `ThirtyDayTrial` クラス + トライアルキー | オフライン判定のみ。**再実装では不要** |
| **Flickr API** | `api.flickr.com/services/rest/` と `/upload/`、frob/token 認証 | 旧 API。**機能自体を対象外** |
| **Locr API** | `www.locr.com/api/*` | サービス終了。**対象外** |
| **Facebook 連携** | Graph 風の投稿 + 自社ホスト（`itravel-tech.com/TGV/webApp/ezTour_V2/gpsTracker/`） | **対象外** |
| Google Earth 起動連携 | `C:\Temp\GoogleEarth` に KMZ を作って Google Earth を呼ぶ | Google Earth Pro がインストールされていれば代替可。**KMZ 出力だけ**残す |

**方針**: これらは「壊れたから直す」対象ではない。**地図は自前、写真共有は無し、
認証は無し**にする。KMZ 出力だけは Google Earth で今も開けるので残す価値がある。

**対象外の明記**: SNS アップロード（Flickr / Locr / Facebook）、
ezTour の Web サイトへのトラック投稿、AGPS データ取得、オンラインサポート、
プロダクトキー認証は、このプロジェクトの**スコープ外**とする。

---

## 8. 再実装するときの最小機能セット（優先順位つき）

ezTour の全機能を写すのではなく、**M-241 のログを読んで地図に出し、
GPX/KML にし、写真に位置を書く**までを最短で通す。

### P0 — これが無いと目的を満たさない

1. **端末ログの読み出しと点列化**（`PMTK182` 経由。既に `m241-core` 側の担当）。
   時刻・緯度・経度・高度・（あれば）速度。
2. **点列のサマリ計算**: 距離・経過時間・平均速度・最高速度・高度の最小/最大。
   ezTour は**点列から再計算**していた（§4.2）。生値が無い場合の代替になる。
3. **GPX 1.1 書き出し**: `trk`/`trkseg`/`trkpt`（lat/lon 9 桁）+ `ele` + `time`。
   creator 文字列は自前の名前にする（vendor 名を名乗らない）。
4. **KML 2.0 / KMZ 書き出し**: `LineString` + 点の `Placemark`、写真を `photo/` に
   同梱、長辺サイズの上限オプション。
5. **地図表示**: 点列ポリライン、全点 fit、現在位置追従（zoom 下限 12 相当）、
   10 色パレット / 幅 5 / 不透明度 0.7 の見た目を踏襲。

### P1 — 写真と編集

6. **写真の取り込みと時刻マッチング**: 最近傍点、上限時間（分）、タイムゾーン、
   距離によるグループ化、時刻シフト後の再マッチング。
7. **EXIF GPS 書き込み**（自前実装）: `GPSVersionID` / lat / lon / alt（+Ref）、
   既存タグの保持、元ファイルのバックアップ。
8. **トラック編集**: 分割・結合・点削除・Undo/Redo。ezTour の操作感
   （Shift 範囲選択 / Ctrl トグル / 矩形選択）は参考にするが**操作体系は
   自前 GUI の流儀に合わせる**。

### P2 — 互換・移行

9. **GPX 1.0 / 1.1 の読み込み**（ezTour や他ツールの出力を取り込むため）。
10. **NMEA 読み書き**: 点列 → `$GPRMC`/`$GPGGA`/`$GPGSA` の再生成（ezTour 互換）。
11. **CSV 書き出し**: 上記の列（UTC/LOCAL 両方、N/S・E/W の文字列表現）。
12. **プロジェクトファイル**: 独自形式でよい（`.itm` 互換は不要）。
    「1 ファイル + 参照写真」の粒度だけ踏襲。
13. **座標形式の切り替え**: 度分秒 / 10 進度 / UTM の 3 つで足りる。
    ezTour の **14 形式（13 か国グリッド + ユーザー定義）・228 データム**は
    中国・台湾・欧州の各国グリッドを詰め込んだ**過剰装備**で、日本のユーザーには
    不要（日本向けグリッドすら無い）。UTM と TWD97/TM2 を足すかは需要次第。

### やらない

- SNS アップロード、Web サイト投稿、AGPS、認証、オンラインヘルプ。
- Yupiteru 等の他社ログ形式の取り込み。
- Google Maps API の再現（自前地図に置き換える）。

---

## 9. 推測に留まったこと（正直な一覧）

| 項目 | 状態 |
|---|---|
| `toTM2` の第 4 引数の役割 | 本文で出力に現れない。**不明** |
| 座標コード 5〜8, 10, 11 の国名 | 定数が各国グリッドの公開定義値と一致したので**同定できた**（Taiwan/Swiss/Sweden/HK80/German/Finnish/British/Austria/RSO/Maidenhead/MGRS/UTM/LatLon）。ただし Finnish(7) だけは定数の同定が弱く、並び順からの推定 |
| データム ID と名前の対応 | **確定した**（`227 − ID`、公開定義値と 18 件一致）。ただし「UI がその ID を選んでいる」ところまでは見ていない |
| ezTour の既定データム / 既定座標形式 | DLL の初期値（`Datum_id=2`, `Coord_id=0`）は ezTour が使う値ではない。アプリ側の既定は**未確定** |
| `HLX_TrackAddPoint` の packed レコード = 端末のログ形式 | ビット配置は読めたが、**端末側フォーマットとの同一性は未確認** |
| `HLX_GetTrkInfo` の速度係数テーブル全表 | 3 系統の係数と 9 個の閾値は読めたが、**どの速度域にどの係数かを式として確定していない** |
| 座標コード 4（Maidenhead）の区画式 | 文字表と基数は読めたが、区画の対応式までは追っていない |
| 「decompile 結果が完全に正しい」こと | 浮動小数点の式は可読な形に崩れている。**式そのものは仕様として写していない** |
| 各サービスの停止時期 | この配布物からは分からない。**外部知識** |
| ezTour が実際に起動して地図が出るか | 未実行（Windows バイナリをこの環境では動かしていない）。**未検証** |
| マニュアルの記述と実装の一致 | マニュアル v2.0 / 実装 v3.0 で版が違う。**一致は未確認** |

---

## 10. 再現手順（この調査をやり直す人向け）

```bash
# 1. 展開（NSIS の自己解凍なので 7z でそのまま出る）
mkdir -p _scratch/eztour/unpacked
7z x -o_scratch/eztour/unpacked _scratch/inbox/holux_eztour_for_logger_v3.0_20170928.zip
# 出てきた HOLUX_ezTour_DL.exe をさらに 7z x する

# 2. export と署名
llvm-readobj --coff-exports _scratch/eztour/unpacked/CoorCvt32.dll
llvm-undname '?Cvt@@YAHHHHPAUcoordinate@@0PA_W1PAUdatum_trans@@PAUTM@@I@Z'

# 3. decompile（Ghidra 12.2 headless。出力は _scratch 側だけに置く）
HOME=/tmp/ghh /opt/ghidra/support/analyzeHeadless /tmp/gez eztour \
  -import "$PWD/_scratch/eztour/CoorCvt32.dll" \
  -scriptPath "$PWD/_scratch/eztour/ghidra_scripts" -postScript Dump.java

# 4. 文字列（ダンプはコミットしない）
strings -el _scratch/eztour/unpacked/ezTour_DL.exe | less
pdftotext -layout _scratch/eztour/unpacked/users_manual.pdf -
```

`_scratch/` は gitignore 済み。vendor バイナリ・文字列ダンプ・Ghidra プロジェクトは
**絶対にコミットしない**（[05-verification.md](05-verification.md) の個人データ規約と
同じ扱い）。
