# CineMantis

<p align="center">
  <img src="apps/desktop/public/cinemantis.png" alt="CineMantis Logo" width="160" />
</p>

<p align="center">
  <strong>ローカル動画ライブラリ管理アプリ</strong><br/>
  MusicBee 風の管理 UI × NAS オフライン耐性 × SQLite ローカル完結
</p>

---

## 概要

CineMantis は、NAS・外付けHDD・ローカルフォルダの動画ファイルを  
「作品」単位で管理するデスクトップアプリです。

- **元ファイルは一切変更しない** — メタデータはすべてローカル SQLite に保存
- **NAS がオフラインでもライブラリ表示・評価・タグ編集が可能**
- **映画・ドラマ・OVA・シリーズ・複数ファイル作品**に対応
- **MusicBee ライクな 3 ペイン UI**（左ナビ / 一覧 / 詳細）

将来的に [Antigravity](https://github.com/tomoich-cmyk) と UI・DB レイヤーを共通化予定。

---

## スクリーンショット

> 開発中につきスクリーンショットは順次追加予定

---

## 技術スタック

| 領域 | 技術 |
|------|------|
| デスクトップフレームワーク | [Tauri 2](https://tauri.app/) |
| フロントエンド | React 18 + TypeScript |
| スタイリング | Tailwind CSS v3 |
| 状態管理 | Zustand + TanStack Query |
| バックエンド | Rust（Tauri コマンド） |
| データベース | SQLite（rusqlite / bundled） |
| 外部メタデータ | TMDb API（v0.4 予定） |
| 動画解析 | ffprobe / ffmpeg |

---

## モノレポ構成

```
CineMantis/
├── apps/
│   └── desktop/            # Tauri 2 アプリ本体
│       ├── src/            # React フロントエンド
│       │   ├── api/        # invoke() ラッパー
│       │   ├── components/ # UI コンポーネント
│       │   ├── hooks/      # TanStack Query hooks
│       │   └── store/      # Zustand ストア
│       └── src-tauri/      # Rust バックエンド
│           └── src/
│               └── commands/ # source / work / stats / tag / scan
├── packages/
│   ├── shared-types/       # 共通 TypeScript 型定義
│   └── db/
│       └── migrations/     # SQLite DDL（001_initial.sql）
└── pnpm-workspace.yaml
```

---

## ドメインモデル

```
Source ──┐
         ├──> File ──> WorkPart ──> Work ──> UserStats
         │                          │
         │                          ├──> WorkTags   ──> Tag
         │                          ├──> WorkPersons ──> Person
         │                          └──> SeriesItems ──> Series
```

| エンティティ | 説明 |
|---|---|
| `Source` | 参照元（NAS / 外付けHDD / ローカル） |
| `File` | 実ファイル（パス・サイズ・コーデック情報） |
| `Work` | 作品単位（映画・ドラマ1話・OVA など） |
| `WorkPart` | 1作品に複数ファイルが紐づく場合の中間単位 |
| `UserStats` | 評価・視聴回数・視聴状態・メモ（オフライン時も編集可） |
| `Tag` | 自由ラベル |
| `Person` | 監督・脚本・出演者 |
| `Series` | シリーズ（007・孤独のグルメ など） |

---

## セットアップ

### 前提条件

- [Node.js 20+](https://nodejs.org/)
- [pnpm 9+](https://pnpm.io/)
- [Rust 1.75+](https://rustup.rs/)
- [Tauri CLI v2](https://tauri.app/start/)
- [ffmpeg / ffprobe](https://ffmpeg.org/) — PATH に通しておく

```bash
# ffmpeg（Windows）
winget install Gyan.FFmpeg
```

### インストール & 起動

```bash
git clone https://github.com/tomoich-cmyk/CineMantis.git
cd CineMantis
pnpm install

# 開発サーバー起動
pnpm tauri dev
```

### ビルド

```bash
pnpm build
```

---

## MVP ロードマップ

| バージョン | 内容 |
|---|---|
| **v0.1** ✅ | ソース登録・スキャン・Works/Files/UserStats・一覧・評価・タグ・ソート・NASオフライン耐性 |
| **v0.2** | サムネイル生成・制作年/国/ジャンル・監督/出演者表示 |
| **v0.3** | Series/SeriesItems・ドラマシリーズ・映画シリーズ・WorkParts |
| **v0.4** | TMDb 連携・手動紐付け UI・locked 運用・ドラマ一括更新 |
| **v0.5** | 一括編集・スマートコレクション・続き再生・プレイリスト |
| **v0.6** | 映画賞・映画祭マスター、受賞/ノミネート管理、作品詳細・賞別一覧 |

---

## 主要画面

### メイン一覧（グリッド / リスト 切替）
- 左ナビ：映画 / ドラマ / シリーズ / 人物 / タグ / 未視聴 / 視聴中 / 最近追加 / 高評価
- 上部：検索・ソート・フィルタ・表示切替
- 右ペイン：詳細（評価・視聴状態・メモ・タグ）

### ソース管理
- フォルダ追加（ローカル / NAS / 外付けHDD）
- スキャン実行（リアルタイム進捗バー）
- オフライン時は `source offline` として扱い、削除しない

---

## Antigravity との連携方針

**独立アプリ + 共通ライブラリ方式**を採用予定。

- 共通化対象：`packages/ui`・`packages/db`・`packages/shared-types`
- 分離対象：動画スキャン・ffprobe/ffmpeg・サムネイル生成・外部メタ取得・再生処理

---

## ライセンス

MIT

---

<p align="center">
  Made with 🦗 by tomoich
</p>
