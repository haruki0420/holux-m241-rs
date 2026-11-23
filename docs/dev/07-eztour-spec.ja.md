# ezTour 仕様化（クリーンルーム）

Holux M-241 のログ読み出し・地図表示・GPX/KML 変換・写真ジオタグを再実装するための参照仕様。対象は **HOLUX ezTour for Logger v3.0**（2017-09-28 ビルド）。

- ベンダーのコード・文字列ダンプ・リソースはリポジトリに置かない（観測結果の要約のみ）。
- 実装状況や未検証の台帳は [05-verification.md](05-verification.md) で管理する。推測は推測と明記する。

## 0. 参照した配布物と確認手段

`_scratch/inbox/holux_eztour_for_logger_v3.0_20170928.zip`（gitignore 対象）を `7z x` で展開（264 ファイル）して調査した。

| 構成物 | 実測 | 役割 |
|---|---|---|
| `ezTour_DL.exe` | 19,288,064 B / PE32 / MFC | 本体 UI・ダイアログ・DLL 呼出・埋め込み Web 資産 |
| `CoorCvt32.dll` | 53,248 B / MSVC8 | 座標変換（8 export） |
| `HLX_GetTrkInfo.dll` | 14,336 B / MSVC8 | 軌跡統計の点別計算（3 export） |
| `exiftool.exe` | 2,933,448 B / v7.17 | 写真 EXIF 書き込み（外部プロセス） |
| `users_manual.pdf` | 24 ページ / v2.0 | ユーザー操作フロー一次資料 |
| `libcef.dll` / `web/` / `icon/` | CEF + 77 ファイル + 116 PNG | 地図 UI・POI アイコン（ActiveX 経路も同梱） |
| `Holux_Driver/` | CP210x VCP（VID 10C4 / PID EA60） | USB シリアルドライバ |

確認手段: `llvm-readobj --coff-exports`、`llvm-undname`、Ghidra 12.2 headless、`objdump`、`.rdata` の Python `<d` デコード、`strings`、`pdftotext -layout`（成果物は `_scratch/` のみ）。

## 1. 全体の流れ（読み込み → 表示 → 変換）

すべてメニュー起点で動作する。

1. **端末から読み出す**（`File > Read Device Log…`）: 「Read log only」または「Read log and import photo」。COM ポート・ボーレート（1200〜115200、M-241 は 38400）は既定で自動スキャン。読み出し後にトラック一覧ダイアログで取り込み対象を選択。
2. **端末設定**（`File > Config GPS…`）: ログモード 4 種（General / Vehicle / Cycling / Hiking）、記録間隔（秒・メートル両方）、メモリ満杯時動作（Overwrite / Stop Log）、`Clear Log`。
3. **一覧・表示・再生**: トラックごとの色・線幅変更、地図再生（`Play`・`Photo Slide Show` 同期）、POI 編集/手動追加、写真の撮影時刻自動マッチングとプレースマーク生成。
4. **編集**: Shift 範囲選択・Ctrl トグル・矩形選択・削除・Split Track・Merge Tracks・複数段 Undo/Redo。`Shift photo time…` で時刻シフト後に自動再マッチング。
5. **保存・書き出し**: プロジェクト保存（`.itm`、SQLite 3。模倣不要だが「1 ファイル＋参照写真」の粒度を参照）、GPX / NMEA / KML / CSV / KMZ / HTML・MHT / RTF 書き出し。
6. **外部サービス**（Flickr / Locr / Facebook）: 本プロジェクトでは**対象外**（§7）。

## 2. 対応フォーマット

| 形式 | 読み | 書き | 備考 |
|---|---|---|---|
| 端末バイナリログ（`.bin`） | ○ | — | シリアルおよびダンプファイル |
| NMEA 0183（`.nmea`/`.NMA`） | ○ | ○ | `$GPRMC` / `$GPGGA` / `$GPGSA` |
| GPX 1.0 / 1.1 | ○ | ○ | 書き出し既定 1.1。`lat`/`lon` 小数 9 桁、`ele`/`time`/`speed`、`wpt`/`rte`、`extensions`、Web 向け簡略化あり |
| KML 2.0 | ○ | ○ | `Document`/`Folder`/`Placemark`、`LineString`/`Point`、`Style`（`LineStyle`/`IconStyle`）、`LookAt`、`lon,lat,alt` |
| KMZ | ○ | ○ | KML + `photo/` + `icon/` を ZIP 化 |
| CSV | — | ○ | 点一覧とレポートの 2 種。12 列: `INDEX`, `TRACK NUMBER`, `UTC DATE`, `UTC TIME`, `LOCAL DATE`, `LOCAL TIME`, `LATITUDE`, `N/S`, `LONGITUDE`, `E/W`, `ALTITUDE`, `SPEED` |
| `.itm` / `.itx` / `.tki` | ○ | ○ | プロジェクト（SQLite 3）/ 旧互換 / トラックキャッシュ |
| `.dst` / Yupiteru（`$YPLOG`） | ○ | — | 目的地リスト / 他社ログ |
| HTML / MHT / RTF | — | ○ | 静的地図スナップショット / 印刷レポート |

## 3. `CoorCvt32.dll` — 8 export の意味

### 3.1 export 一覧
`Coord_id`（変数、初期値 0）、`Datum_id`（変数、初期値 2）、`Cvt`（緯度経度→指定形式＋ワイド文字列 2 本）、`CvtRvs` / `CvtRvs2` / `CvtRvs2WGS84`（逆変換）、`UTM2LL`、`toTM2`（横メルカトル正投影）。
**ezTour 本体が import しているのは `Cvt` のみ**。呼び出しごとに座標形式とデータムを引数で渡すため、DLL の初期値は ezTour の既定表示ではない。

### 3.2 構造体
- `coordinate`: 24 バイト（`double` ×3: lat, lon, alt）。
- `datum_trans`: `double` ×2（長半径 `a`、逆扁平率 `1/f`）。
- `TM`: `double` ×5 以上（中央子午線・原点緯度・縮尺・偽東距・偽北距と推定）。`Cvt` 第 10 引数（呼び出し側は `0x3A8`）は未使用。

### 3.3 座標形式コード（`Coord_id` 0〜13）
0: Lat/Lon（DMS/10進度）、1: UTM（`k0=0.9996`, FE `500000`, 南半球 `+10,000,000`）、2: MGRS、3: RSO（マレー半島と推定、FE `804671.2996`）、4: Maidenhead Locator（8 文字）、5: Austria Grid、6: British Grid（OSGB36 `k0=0.9996012717`）、7: Finnish Grid（KKJ と整合・推定寄り）、8: German Grid（Gauss-Krüger）、9: HK80、10: Sweden Grid（RT90）、11: Swiss Grid（CH1903）、12: Taiwan Grid（TWD97 TM2 121°E）、13: User Defined（`TM*` 必須）。日本向けグリッドは含まれない。

### 3.4 データム（`Datum_id`）
`.rdata` テーブル（`0x100072A8`）に **228 件（ID 0〜227）**、32 バイト/件（楕円体ポインタ + `dx, dy, dz` の 3 パラメータ平行移動）。ID `0xDF`（223, WGS 1972）は変換スキップ、ID `0xE4`（228）はユーザー定義（`datum_trans*` 必須）。ID `n` は UI 名前一覧の `227 − n` 番目に対応（`WGS 1984`=224、`TWD'97`=218、`TWD'67`=217、`Tokyo-Japan` 等も収録）。アプリ側既定値は未確定。

### 3.5 `toTM2` の引数
`toTM2(a, 1/f, <deg>, <deg:役割不明>, <deg>, <deg>, k0, double* east, double* north)`。2 度帯・6 度帯兼用の横メルカトル実装。

### 3.6 数値の丸め・表示規則
- 度分秒: `%c %3d^%02d'%06.3f"` / 度分: `%c %3d^%08.5f'` / 10 進度: `%c %12.8f^`（`^` を `°` へ置換、負号でなく半球文字 `N/S/E/W`）。
- 東距・北距: RSO は小数 3 桁（`%6.3fE`/`N`）、UTM・各国グリッドは整数 m（`%7dE`/`N`）。

## 4. `HLX_GetTrkInfo.dll` — 3 export

ezTour 本体が 3 つすべてを呼び出す: `HLX_TrackInit(4, 650)`（`0x140` バイト確保）、`HLX_TrackAddPoint(HLX_POINT*, HLX_TRKINFO*)`、`HLX_TrackExit()`。

### 4.1 `HLX_TrackAddPoint` の処理
- 36 バイト packed レコードを組み立てる（日時は 31bit packed: 年は 2000 起点、5+4+5+5+6+6 bit。端末ログ形式との同一性は未確認）。
- 開始/終了/現在日時、移動距離、速度、平均速度、最高速度、経過時間、高度を `HLX_TRKINFO` に書き出す。日跨ぎ（86400 秒）ラップ、速度域別係数（全表未取得）、7 統計量による平滑化痕跡（推測）を持つ。

### 4.2 再実装への示唆
ezTour の速度・距離・標高サマリは端末生値ではなく点列からの再計算値を使っている（[06-open-questions.md](06-open-questions.md) 参照）。

## 5. 地図表示の要件

`libcef.dll` 上で Google Maps JS API v2/v3（現在は停止・キー無効）を使用していた。再実装で引き継ぐ振る舞い:
- **軌跡**: 既定 10 色パレット巡回、線幅 **5 px**、不透明度 **0.7**（トラックごとの色・幅変更可）。
- **視界**: 全点 `fitBounds`、追従時は現在位置へ `panTo`（**zoom 12 未満なら 12 に上げる**）。
- **マーカー**: 始点（`line_start`）・終点（`line_end`）・現在位置・POI・写真（`camera`）。
- **再生**: 点列に沿った位置移動と写真スライドショー同期。
- **HTML 書き出し**: CSS/JS インライン展開による単一ファイル出力。

## 6. 写真ジオタグの仕様

### 6.1 マッチングの規則
1. EXIF `DateTimeOriginal` を読む。
2. カメラ側タイムゾーン（既定 PC タイムゾーン、夏時間別設定）で UTC へ変換。
3. 軌跡の最近傍点を探す。
4. 差が `maxPhotoDiffTime`（分）を超えたらマッチさせない。
5. 2 枚間の距離が `groupPhotoDistance`（m）未満なら同一プレースマークへまとめる。
6. 表示用タイムゾーンは表示のみに効く。
7. `Shift photo time…`（日/時/分/秒）は適用範囲 3 種（`allPhotos` / `selPhotos` / `byCamera`）を持ち、シフト後に自動再マッチングする。
8. GPS 情報のない写真も取り込める。

### 6.2 書き込む EXIF
同梱 `exiftool.exe` を外部起動して GPS IFD を書き込む（JPEG/TIFF 対象、MP4 等は紐づけのみ）。
- タグ: `GPSVersionID`（ezTour は `0.0.2.2` を渡しているが規格順 `2.2.0.0` と逆。理由は不明）、`GPSLatitudeRef`/`LongitudeRef`（`N`/`S`/`E`/`W`）、`GPSLatitude`/`Longitude`（10 進度 `%f`）、`GPSAltitudeRef`（海抜/海面下）、`GPSAltitude`（m）。
- `-TagsFromFile <src> -all:all <dst>` で既存タグを保持し、`_original` バックアップを残す。時刻シフト時は `DateTimeOriginal` も書き換える。

### 6.3 KMZ に同梱する写真
`photo/` に写真、`icon/` にサムネイルを格納。`kmzMaxPhotoEdgeSize`（px）で長辺を縮小して ZIP 化する。

## 7. 死んでいる依存（対象外）

Google Maps JS API v2/v3（旧キー無効・サービス停止）、holux.com / iTravel-Tech プロダクトキー認証サーバ（終了）、30 日トライアル、Flickr / Locr / Facebook 連携、AGPS、オンラインサポートは**スコープ外**とし再実装しない。Google Earth 連携は KMZ 出力のみ残す。

## 8. 再実装の最小機能セット（優先順位）

- **P0**: 1. 端末ログ読出・点列化（PMTK182） / 2. 点列サマリ再計算 / 3. GPX 1.1 書き出し（9 桁、自前 creator） / 4. KML 2.0・KMZ 書き出し（`photo/`・長辺上限） / 5. 地図表示（10 色・幅 5・不透明度 0.7・全点 fit・追従 zoom ≥ 12）。
- **P1**: 6. 写真取込・時刻マッチ・距離グループ化・時刻シフト再マッチ / 7. 自前 EXIF GPS 書込（5 タグ・既存保持・バックアップ） / 8. トラック編集（分割・結合・削除・Undo/Redo）。
- **P2**: 9. GPX 1.0/1.1 読込 / 10. NMEA 読み書き / 11. CSV 書き出し / 12. 独自プロジェクトファイル（1 ファイル＋参照写真） / 13. 座標形式切替（度分秒・10 進度・UTM の 3 種。各国グリッド・228 データムは過剰装備のため不要）。
- **やらない**: SNS 投稿、認証、AGPS、Yupiteru 等他社形式、Google Maps API 再現。

## 9. 推測・未確認の一覧

`toTM2` 第 4 引数（不明）、Finnish(7) の定数同定（並び順推定寄り）、UI とデータム ID の実呼出、アプリ側既定データム/座標形式、`HLX_TrackAddPoint` packed レコードと端末ログの同一性、`HLX_GetTrkInfo` 速度係数全表、Maidenhead 区画式、decompile 浮動小数点式の厳密性、サービス停止時期（外部知識）、ezTour 実機起動（未実行）、マニュアル v2.0 と実装 v3.0 の差異。

## 10. 再現手順

`_scratch/` 配下で `7z x` 展開 → `llvm-readobj --coff-exports` / `llvm-undname` → Ghidra 12.2 headless → `strings -el` / `pdftotext -layout`。ベンダーバイナリ・ダンプ・Ghidra プロジェクトは絶対にコミットしない。
