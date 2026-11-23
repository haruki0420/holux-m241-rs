# 並列作業の分割と決まり

並列ストリームでのファイル所有権と共通規約の記録。現在の進捗と次期ロードマップは [`12-progress-summary.md`](12-progress-summary.md) を参照。

## 完了済みストリーム（`main` 統合・三段検証済み）

土台・読み書き分離、DA 逆解析（[`01-da-protocol.md`](../reference/01-da-protocol.md)）、ezTour 仕様化（[`07-eztour-spec.ja.md`](07-eztour-spec.ja.md)）、先行実装差分（[`04-prior-art.md`](04-prior-art.md)）、コア読み取り・変換・EXIF/ジオタグ、DA 書き込み実装、CLI 全 17 コマンド、egui GUI 4 画面、Tauri v2 P0〜P2。

## 並列作業の共通規約

- 担当範囲のファイルのみ編集し、共通ファイル（`Cargo.toml`・`docs/dev/*`）は統合時に更新する。
- `./tools/check.sh` 6/6 でコミットする（テスト外の `unwrap` / `expect` / `panic` / `unsafe` / 無検証添字は禁止）。
- コメントは「なぜ」を書く。実機未確認は「未検証」と明記し、[`05-verification.md`](05-verification.md) のみで昇格させる。
- 個人データ・ベンダーバイナリをコミットしない。終了したワークツリーと `target/` は片付ける（`AGENTS.md` §9.6）。
- レビューは [`09-review-protocol.md`](09-review-protocol.md) と `AGENTS.md` §8 の三段構成（発見 → 別エージェント修正 → 三人目が説明未読で独立検証）に従う。
