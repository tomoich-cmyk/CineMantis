import { useState } from "react";
import { clsx } from "clsx";
import { bulkKeys, useAttentionStats } from "@/hooks/useBulk";
import { useAutoMatchWork, useRefreshTmdbMetadata } from "@/hooks/useTmdb";
import { CandidateDialog } from "@/components/tmdb/CandidateDialog";
import { listWorks } from "@/api/works";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { WorkSummary } from "@cinemantis/shared-types";
import { useLibraryStore } from "@/store/libraryStore";

// ─── 要確認カテゴリ定義 ───────────────────────────────────────────────────────

interface AttentionCategory {
  key: string;
  label: string;
  icon: string;
  description: string;
  count: (stats: ReturnType<typeof useAttentionStats>["data"]) => number;
  action?: string; // アクション例（ツールチップ）
}

const CATEGORIES: AttentionCategory[] = [
  {
    key: "unmatched",
    label: "未照合",
    icon: "🔍",
    description: "TMDb 情報が紐付いていない作品",
    count: (s) => s?.unmatched ?? 0,
    action: "自動照合 or 候補を手動選択",
  },
  {
    key: "missing_meta",
    label: "メタ不足",
    icon: "📋",
    description: "製作年またはジャンルが未入力",
    count: (s) => s?.missing_meta ?? 0,
    action: "メタデータ再取得",
  },
  {
    key: "no_persons",
    label: "人物なし",
    icon: "👤",
    description: "監督・出演情報が未取得",
    count: (s) => s?.no_persons ?? 0,
    action: "メタデータ再取得で自動登録",
  },
  {
    key: "file_missing",
    label: "ファイル消失",
    icon: "💔",
    description: "登録済みファイルが見つからない",
    count: (s) => s?.file_missing ?? 0,
    action: "ソースを再スキャンまたはファイル確認",
  },
];

// ─── 要確認作品リスト ─────────────────────────────────────────────────────────

function useAttentionWorks(filter: string | null) {
  return useQuery({
    queryKey: ["attention-works", filter],
    queryFn: () =>
      listWorks({
        attentionFilter: filter!,
        sortField: "created_at",
        sortOrder: "desc",
      }),
    enabled: filter !== null,
  });
}

function WorkMiniCard({
  work,
  active,
  onSelect,
}: {
  work: WorkSummary;
  active: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      onClick={onSelect}
      className={clsx(
        "flex items-center gap-2 px-3 py-2 rounded transition-colors text-left w-full border",
        active
          ? "bg-yellow-900/20 border-yellow-700/70"
          : "bg-surface border-subtle hover:bg-surface-hover hover:border-gray-600"
      )}
    >
      <span className="text-lg">
        {work.matchStatus === "unmatched" ? "🔍" : "🎬"}
      </span>
      <div className="flex-1 min-w-0">
        <p className="text-xs font-medium text-gray-200 truncate">{work.title}</p>
        <p className="text-[11px] text-gray-600">{work.year ?? "—"}</p>
      </div>
    </button>
  );
}

// ─── AttentionCenterScreen ────────────────────────────────────────────────────

export function AttentionCenterScreen() {
  const { data: stats, isLoading: statsLoading } = useAttentionStats();
  const [selectedFilter, setSelectedFilter] = useState<string | null>(null);
  const [selectedWorkId, setSelectedWorkIdLocal] = useState<number | null>(null);
  const [showCandidates, setShowCandidates] = useState(false);
  const { data: filteredWorks = [], isLoading: worksLoading } = useAttentionWorks(selectedFilter);
  const qc = useQueryClient();
  const autoMatch = useAutoMatchWork();
  const refreshMeta = useRefreshTmdbMetadata();
  const { setSelectedWorkId, setActiveSection } = useLibraryStore();

  const selectedCategory = CATEGORIES.find((c) => c.key === selectedFilter);
  const selectedWork = filteredWorks.find((work) => work.id === selectedWorkId) ?? null;
  const totalIssues = CATEGORIES.reduce((sum, c) => sum + c.count(stats), 0);
  const isMatched = selectedWork
    ? ["auto", "manual", "locked", "matched"].includes(selectedWork.matchStatus)
    : false;

  function refreshAttentionQueries() {
    qc.invalidateQueries({ queryKey: ["attention-works"] });
    qc.invalidateQueries({ queryKey: bulkKeys.attentionStats });
  }

  function openWork(workId: number) {
    setSelectedWorkId(workId);
    setActiveSection("all-movies");
  }

  function selectFilter(key: string) {
    setSelectedFilter((current) => (current === key ? null : key));
    setSelectedWorkIdLocal(null);
    setShowCandidates(false);
  }

  function runAutoMatch(workId: number) {
    autoMatch.mutate(workId, {
      onSuccess: () => refreshAttentionQueries(),
    });
  }

  function runRefreshMetadata(workId: number) {
    refreshMeta.mutate(workId, {
      onSuccess: () => refreshAttentionQueries(),
    });
  }

  return (
    <div className="flex-1 overflow-y-auto p-6">
      <div className="max-w-3xl mx-auto">
        {/* ヘッダー */}
        <div className="flex items-start justify-between mb-6">
          <div>
            <h1 className="text-lg font-semibold text-gray-100">要確認センター</h1>
            <p className="text-xs text-gray-500 mt-0.5">
              ライブラリの整備が必要な項目を確認・修正できます
            </p>
          </div>
          {!statsLoading && totalIssues > 0 && (
            <span className="text-xs px-2.5 py-1 bg-yellow-900/30 border border-yellow-800/60 text-yellow-500 rounded-full">
              {totalIssues} 件の要確認項目
            </span>
          )}
          {!statsLoading && totalIssues === 0 && (
            <span className="text-xs px-2.5 py-1 bg-mantis-900/40 border border-mantis-700/60 text-mantis-400 rounded-full">
              ✓ 問題なし
            </span>
          )}
        </div>

        {/* カテゴリカード群 */}
        <div className="grid grid-cols-2 gap-3 mb-6">
          {CATEGORIES.map((cat) => {
            const count = cat.count(stats);
            const isActive = selectedFilter === cat.key;
            return (
              <button
                key={cat.key}
                onClick={() => selectFilter(cat.key)}
                disabled={count === 0 && !isActive}
                className={clsx(
                  "flex items-start gap-3 p-4 rounded border text-left transition-all",
                  isActive
                    ? "bg-yellow-900/20 border-yellow-700/60"
                    : count > 0
                    ? "bg-surface-elevated border-subtle hover:border-yellow-800/60 hover:bg-yellow-900/10"
                    : "bg-surface border-subtle opacity-40 cursor-default"
                )}
              >
                <span className="text-2xl leading-none mt-0.5">{cat.icon}</span>
                <div className="flex-1 min-w-0">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-sm font-medium text-gray-200">{cat.label}</span>
                    <span className={clsx(
                      "text-sm font-bold",
                      count > 0 ? "text-yellow-400" : "text-mantis-500"
                    )}>
                      {statsLoading ? "…" : count}
                    </span>
                  </div>
                  <p className="text-xs text-gray-500 mt-0.5 leading-tight">{cat.description}</p>
                  {cat.action && count > 0 && (
                    <p className="text-[11px] text-gray-700 mt-1">→ {cat.action}</p>
                  )}
                </div>
              </button>
            );
          })}
        </div>

        {/* フィルタ済み作品リスト */}
        {selectedFilter && (
          <div className="border border-subtle rounded-lg overflow-hidden">
            <div className="flex items-center justify-between px-4 py-2.5 bg-surface-elevated border-b border-subtle">
              <span className="text-sm font-medium text-gray-200">
                {selectedCategory?.icon} {selectedCategory?.label} — {filteredWorks.length} 件
              </span>
              <button
                onClick={() => {
                  setSelectedFilter(null);
                  setSelectedWorkIdLocal(null);
                  setShowCandidates(false);
                }}
                className="text-gray-600 hover:text-gray-300 text-lg transition-colors"
              >
                ×
              </button>
            </div>

            {worksLoading ? (
              <div className="p-6 text-center text-gray-600 text-sm animate-pulse">読み込み中…</div>
            ) : filteredWorks.length === 0 ? (
              <div className="p-6 text-center text-gray-600 text-sm">
                ✓ 対象の作品はありません
              </div>
            ) : (
              <div className="grid grid-cols-2 gap-2 p-3 max-h-96 overflow-y-auto">
                {filteredWorks.map((work) => (
                  <WorkMiniCard
                    key={work.id}
                    work={work}
                    active={selectedWorkId === work.id}
                    onSelect={() => setSelectedWorkIdLocal(work.id)}
                  />
                ))}
              </div>
            )}

            {selectedWork && (
              <div className="border-t border-subtle bg-surface-elevated px-4 py-3">
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0">
                    <p className="text-xs text-gray-500">選択中</p>
                    <p className="truncate text-sm font-semibold text-gray-100">
                      {selectedWork.title}
                    </p>
                    <p className="mt-0.5 text-xs text-gray-600">
                      {selectedWork.year ?? "年不明"} / {selectedWork.matchStatus}
                    </p>
                  </div>
                  <div className="flex flex-wrap justify-end gap-2">
                    <button
                      onClick={() => setShowCandidates(true)}
                      className="rounded border border-mantis-700 bg-mantis-900/30 px-3 py-1.5 text-xs text-mantis-300 hover:bg-mantis-800/40"
                    >
                      候補を探す
                    </button>
                    {!isMatched && (
                      <button
                        onClick={() => runAutoMatch(selectedWork.id)}
                        disabled={autoMatch.isPending}
                        className="rounded border border-yellow-800 bg-yellow-900/20 px-3 py-1.5 text-xs text-yellow-300 hover:bg-yellow-900/35 disabled:opacity-40"
                      >
                        {autoMatch.isPending ? "照合中…" : "自動照合"}
                      </button>
                    )}
                    <button
                      onClick={() => runRefreshMetadata(selectedWork.id)}
                      disabled={!isMatched || refreshMeta.isPending}
                      title={!isMatched ? "先にTMDb照合してください" : undefined}
                      className="rounded border border-subtle px-3 py-1.5 text-xs text-gray-300 hover:border-gray-600 disabled:opacity-35"
                    >
                      {refreshMeta.isPending ? "取得中…" : "メタデータ再取得"}
                    </button>
                    <button
                      onClick={() => openWork(selectedWork.id)}
                      className="rounded border border-subtle px-3 py-1.5 text-xs text-gray-500 hover:text-gray-200"
                    >
                      ライブラリで開く
                    </button>
                  </div>
                </div>
                {autoMatch.error && (
                  <p className="mt-2 text-xs text-red-400">
                    自動照合に失敗しました: {String(autoMatch.error)}
                  </p>
                )}
                {refreshMeta.error && (
                  <p className="mt-2 text-xs text-red-400">
                    メタデータ再取得に失敗しました: {String(refreshMeta.error)}
                  </p>
                )}
              </div>
            )}
          </div>
        )}
      </div>

      {selectedWork && showCandidates && (
        <CandidateDialog
          workId={selectedWork.id}
          workTitle={selectedWork.title}
          isLocked={selectedWork.matchStatus === "locked"}
          onClose={() => {
            setShowCandidates(false);
            refreshAttentionQueries();
          }}
        />
      )}
    </div>
  );
}
