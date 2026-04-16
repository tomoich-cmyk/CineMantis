import { useState, useEffect } from "react";
import { getVersion, getName, getTauriVersion } from "@tauri-apps/api/app";
import { appDataDir } from "@tauri-apps/api/path";
import { invoke } from "@tauri-apps/api/core";

// DB スキーマバージョン: マイグレーション追加時に手動でインクリメント
const DB_SCHEMA_VERSION = "1";

// ─── ユーティリティ ───────────────────────────────────────────────────────────

async function copyToClipboard(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // フォールバック
    const el = document.createElement("textarea");
    el.value = text;
    document.body.appendChild(el);
    el.select();
    document.execCommand("copy");
    document.body.removeChild(el);
  }
}

// ─── デスクトップショートカット作成ボタン ─────────────────────────────────────

function CreateShortcutButton() {
  const [state, setState] = useState<"idle" | "ok" | "err">("idle");
  const [errMsg, setErrMsg] = useState("");

  async function handleCreate() {
    setState("idle");
    try {
      await invoke("create_desktop_shortcut");
      setState("ok");
      setTimeout(() => setState("idle"), 3000);
    } catch (e) {
      setErrMsg(String(e));
      setState("err");
      setTimeout(() => setState("idle"), 5000);
    }
  }

  return (
    <div className="flex flex-col gap-1.5">
      <button
        onClick={handleCreate}
        disabled={state !== "idle"}
        className="flex items-center gap-2 px-4 py-2 text-sm border border-subtle rounded-lg
                   bg-surface-elevated hover:bg-surface-hover transition-colors
                   text-gray-300 hover:text-gray-100 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        <span className="text-base">🖥</span>
        <span>デスクトップにショートカットを作成</span>
        {state === "ok"  && <span className="ml-auto text-mantis-400 text-xs">✓ 作成しました</span>}
        {state === "err" && <span className="ml-auto text-red-400 text-xs">✗ 失敗</span>}
      </button>
      {state === "err" && errMsg && (
        <p className="text-[11px] text-red-500 px-1 font-mono break-all">{errMsg}</p>
      )}
    </div>
  );
}

// ─── コピーボタン付きパス表示 ─────────────────────────────────────────────────

function PathRow({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);

  async function handleCopy() {
    await copyToClipboard(value);
    setCopied(true);
    setTimeout(() => setCopied(false), 1800);
  }

  return (
    <div className="flex items-start gap-3 py-2 border-b border-subtle last:border-0">
      <span className="text-xs text-gray-500 w-28 flex-shrink-0 pt-0.5">{label}</span>
      <span className="text-xs text-gray-300 font-mono break-all flex-1 leading-relaxed">
        {value}
      </span>
      <button
        onClick={handleCopy}
        title="パスをコピー"
        className="text-xs text-gray-600 hover:text-gray-300 transition-colors flex-shrink-0 px-1.5 py-0.5 rounded border border-transparent hover:border-subtle"
      >
        {copied ? "✓" : "⎘"}
      </button>
    </div>
  );
}

// ─── テックスタック行 ─────────────────────────────────────────────────────────

const TECH_STACK = [
  { name: "Tauri 2",          role: "デスクトップフレームワーク" },
  { name: "React 18",         role: "UI ライブラリ" },
  { name: "TypeScript 5",     role: "フロントエンド言語" },
  { name: "Rust",             role: "バックエンド言語" },
  { name: "SQLite",           role: "ローカルデータベース (rusqlite)" },
  { name: "TanStack Query 5", role: "サーバー状態管理" },
  { name: "Zustand 4",        role: "クライアント状態管理" },
  { name: "Tailwind CSS 3",   role: "スタイリング" },
  { name: "TMDb API",         role: "映画・TVメタデータ" },
];

// ─── メイン画面 ───────────────────────────────────────────────────────────────

export function AboutScreen() {
  const [appVersion,    setAppVersion]    = useState<string>("…");
  const [tauriVersion,  setTauriVersion]  = useState<string>("…");
  const [appName,       setAppName]       = useState<string>("CineMantis");
  const [dataDir,       setDataDir]       = useState<string>("…");

  useEffect(() => {
    getVersion()   .then(setAppVersion)   .catch(() => setAppVersion("?"));
    getTauriVersion().then(setTauriVersion).catch(() => setTauriVersion("?"));
    getName()      .then(setAppName)      .catch(() => {});
    appDataDir()   .then(setDataDir)      .catch(() => setDataDir("取得できませんでした"));
  }, []);

  const dbPath = dataDir !== "…" && dataDir !== "取得できませんでした"
    ? `${dataDir.replace(/[/\\]$/, "")}/cinemantis.db`
    : "…";

  const cachePath = dataDir !== "…" && dataDir !== "取得できませんでした"
    ? `${dataDir.replace(/[/\\]$/, "")}/cache`
    : "…";

  return (
    <div className="flex-1 overflow-y-auto p-8">
      <div className="max-w-lg mx-auto flex flex-col gap-8">

        {/* ── ヘッダー ── */}
        <div className="flex flex-col items-center text-center gap-3">
          {/* ロゴ代わりの絵文字 + グラデーションテキスト */}
          <div className="text-6xl select-none">🪲</div>
          <div>
            <h1 className="text-2xl font-bold tracking-wide text-gray-100">
              {appName}
            </h1>
            <p className="text-sm text-mantis-400 font-medium mt-0.5">
              Video Library Manager
            </p>
          </div>
          <div className="flex items-center gap-2 text-xs text-gray-500">
            <span className="px-2 py-0.5 rounded-full bg-mantis-900/40 border border-mantis-800/60 text-mantis-400 font-mono font-semibold">
              v{appVersion}
            </span>
            <span className="text-gray-700">·</span>
            <span>Tauri {tauriVersion}</span>
            <span className="text-gray-700">·</span>
            <span>DB schema {DB_SCHEMA_VERSION}</span>
          </div>
        </div>

        {/* ── デスクトップショートカット ── */}
        <CreateShortcutButton />

        {/* ── データパス ── */}
        <section>
          <h2 className="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-2">
            データパス
          </h2>
          <div className="bg-surface-elevated border border-subtle rounded-lg px-4 py-1">
            <PathRow label="アプリデータ"   value={dataDir} />
            <PathRow label="データベース"   value={dbPath} />
            <PathRow label="キャッシュ"     value={cachePath} />
          </div>
          <p className="text-[11px] text-gray-700 mt-1.5 px-1">
            ⎘ でパスをクリップボードにコピーできます
          </p>
        </section>

        {/* ── 機能一覧 ── */}
        <section>
          <h2 className="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-2">
            主な機能
          </h2>
          <div className="grid grid-cols-2 gap-1.5">
            {[
              ["📂", "ソーススキャン",     "動画ファイルの自動検出"],
              ["🎞", "メタデータ照合",     "TMDb による自動マッチング"],
              ["🖼", "サムネ/ポスター",    "ffmpeg 自動生成 + TMDb"],
              ["🔍", "Discover",          "フィルタ・スマート棚"],
              ["▶",  "Watch",             "OS デフォルトプレイヤー連携"],
              ["✓",  "Operate",           "一括編集・タグ・マッチング"],
              ["🛡",  "Protect",          "バックアップ/リストア"],
              ["🔬", "Audit",             "重複検出・整合性チェック"],
            ].map(([icon, name, desc]) => (
              <div
                key={name}
                className="flex items-start gap-2 px-3 py-2 bg-surface-elevated border border-subtle rounded"
              >
                <span className="text-sm flex-shrink-0 mt-0.5">{icon}</span>
                <div>
                  <p className="text-xs font-medium text-gray-300">{name}</p>
                  <p className="text-[11px] text-gray-600 leading-snug">{desc}</p>
                </div>
              </div>
            ))}
          </div>
        </section>

        {/* ── テックスタック ── */}
        <section>
          <h2 className="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-2">
            テックスタック
          </h2>
          <div className="bg-surface-elevated border border-subtle rounded-lg divide-y divide-subtle">
            {TECH_STACK.map(({ name, role }) => (
              <div key={name} className="flex items-center justify-between px-4 py-2">
                <span className="text-xs font-mono text-gray-300">{name}</span>
                <span className="text-[11px] text-gray-600">{role}</span>
              </div>
            ))}
          </div>
        </section>

        {/* ── 既知の制約 ── */}
        <section>
          <h2 className="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-2">
            既知の制約
          </h2>
          <ul className="bg-surface-elevated border border-subtle rounded-lg divide-y divide-subtle text-[11px] text-gray-500">
            {[
              "再生はOS標準プレイヤーに委譲（シーク同期は手動）",
              "TMDb API キーの別途取得が必要",
              "動画サムネイル生成には ffmpeg / ffprobe が必要",
              "グリッドビューは仮想化なし（数千件では重くなる場合あり）",
              "ネットワークドライブ上のファイルは availability_status が変動する",
            ].map((text) => (
              <li key={text} className="px-4 py-2 flex items-start gap-2">
                <span className="text-yellow-700 flex-shrink-0">•</span>
                <span>{text}</span>
              </li>
            ))}
          </ul>
        </section>

        {/* ── フッター ── */}
        <div className="text-center text-[11px] text-gray-700 pb-4">
          CineMantis {appVersion} — Built with Tauri 2 + React + Rust
        </div>

      </div>
    </div>
  );
}
