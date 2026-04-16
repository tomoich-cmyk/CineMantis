import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { setSetting, getTmdbApiKeyMasked } from "@/api/settings";
import { autoMatchSource } from "@/api/tmdb";

export function SettingsScreen() {
  const qc = useQueryClient();

  // ── TMDb APIキー ──────────────────────────────────────────────────────────
  const { data: maskedKey } = useQuery({
    queryKey: ["settings", "tmdb_api_key_masked"],
    queryFn: getTmdbApiKeyMasked,
  });

  const [inputKey, setInputKey] = useState("");
  const [showInput, setShowInput] = useState(false);

  const { mutate: saveKey, isPending: savingKey } = useMutation({
    mutationFn: (key: string) => setSetting("tmdb_api_key", key),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["settings"] });
      setInputKey("");
      setShowInput(false);
    },
  });

  // ── 全件一括照合 ──────────────────────────────────────────────────────────
  const [matching, setMatching] = useState(false);
  const [matchResult, setMatchResult] = useState<{
    matched: number; total: number; failed: number;
  } | null>(null);

  async function handleBulkMatch() {
    setMatching(true);
    setMatchResult(null);
    try {
      const r = await autoMatchSource(null);
      setMatchResult({ matched: r.matched, total: r.total, failed: r.failed });
    } finally {
      setMatching(false);
    }
  }

  return (
    <div className="flex-1 overflow-y-auto p-6">
      <div className="max-w-xl mx-auto flex flex-col gap-8">
        <h1 className="text-lg font-semibold text-gray-100">設定</h1>

        {/* ── TMDb API ── */}
        <section className="flex flex-col gap-3">
          <div>
            <h2 className="text-sm font-medium text-gray-200">TMDb API キー</h2>
            <p className="text-xs text-gray-500 mt-0.5">
              映画・ドラマのメタデータ照合に使います。
              <a
                href="https://www.themoviedb.org/settings/api"
                target="_blank"
                rel="noopener noreferrer"
                className="text-blue-500 hover:text-blue-400 ml-1"
              >
                TMDb でキーを取得 ↗
              </a>
            </p>
          </div>

          <div className="flex items-center gap-3 p-3 bg-surface rounded border border-subtle">
            {maskedKey ? (
              <div className="flex items-center gap-2 flex-1">
                <span className="text-mantis-400 text-sm">✓</span>
                <span className="text-gray-400 font-mono text-sm">{maskedKey}</span>
              </div>
            ) : (
              <span className="text-gray-600 text-sm flex-1">未設定</span>
            )}
            <button
              onClick={() => setShowInput(!showInput)}
              className="text-xs text-gray-500 hover:text-gray-300 border border-subtle rounded px-2 py-1 transition-colors"
            >
              {maskedKey ? "変更" : "設定"}
            </button>
          </div>

          {showInput && (
            <div className="flex gap-2">
              <input
                type="password"
                value={inputKey}
                onChange={(e) => setInputKey(e.target.value)}
                placeholder="API キーを入力…"
                className="flex-1 bg-surface border border-subtle rounded px-3 py-1.5 text-sm text-gray-200 placeholder-gray-700 outline-none focus:border-mantis-600 font-mono"
              />
              <button
                onClick={() => saveKey(inputKey)}
                disabled={!inputKey || savingKey}
                className="px-4 py-1.5 text-sm bg-mantis-700 hover:bg-mantis-600 text-white rounded transition-colors disabled:opacity-40"
              >
                {savingKey ? "保存中…" : "保存"}
              </button>
            </div>
          )}
        </section>

        {/* ── メタデータ照合 ── */}
        <section className="flex flex-col gap-3">
          <div>
            <h2 className="text-sm font-medium text-gray-200">メタデータ一括照合</h2>
            <p className="text-xs text-gray-500 mt-0.5">
              未照合の作品すべてに対して TMDb 照合を実行します。
              信頼度 90 以上の候補が自動適用されます。
            </p>
          </div>

          <button
            onClick={handleBulkMatch}
            disabled={matching || !maskedKey}
            className="self-start px-4 py-2 text-sm border border-mantis-700 text-mantis-400 hover:bg-mantis-900/30 rounded transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {matching ? "照合中…" : "全件照合を実行"}
          </button>

          {!maskedKey && (
            <p className="text-xs text-yellow-600">
              TMDb API キーを設定してください
            </p>
          )}

          {matchResult && (
            <div className="flex items-center gap-3 text-xs p-3 bg-surface rounded border border-subtle">
              <span className="text-mantis-400">完了</span>
              <span className="text-gray-400">
                対象 {matchResult.total} 件 / 照合成功 {matchResult.matched} 件 / 失敗 {matchResult.failed} 件
              </span>
            </div>
          )}
        </section>

        {/* ── アプリ情報 ── */}
        <section className="flex flex-col gap-2 border-t border-subtle pt-6">
          <h2 className="text-sm font-medium text-gray-500">アプリ情報</h2>
          <div className="text-xs text-gray-700 space-y-1">
            <div className="flex gap-4">
              <span className="w-24">バージョン</span>
              <span>0.1.0</span>
            </div>
            <div className="flex gap-4">
              <span className="w-24">スタック</span>
              <span>Tauri 2 + React + SQLite</span>
            </div>
          </div>
        </section>
      </div>
    </div>
  );
}
