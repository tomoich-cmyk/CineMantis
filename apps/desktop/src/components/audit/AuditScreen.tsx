import { useState } from "react";
import { clsx } from "clsx";
import {
  useDuplicateGroups,
  useIntegrityReport,
  useDeleteWork,
  useRepairFetchPersons,
  useRefreshTmdbMetadata,
} from "@/hooks/useAudit";
import type { DuplicateGroup, IntegrityIssue, IssueCode } from "@/api/audit";
import { useLibraryStore } from "@/store/libraryStore";
import { CandidateDialog } from "@/components/tmdb/CandidateDialog";

// ─── ラベルマップ ─────────────────────────────────────────────────────────────

const ISSUE_LABELS: Record<IssueCode, { label: string; icon: string; desc: string }> = {
  unmatched:    { label: "未照合",     icon: "🔍", desc: "TMDb 情報が紐付いていない作品" },
  missing_meta: { label: "メタ不足",   icon: "📋", desc: "製作年またはジャンルが未入力" },
  no_persons:   { label: "人物なし",   icon: "👤", desc: "監督・出演情報が未取得" },
  file_missing: { label: "ファイル消失", icon: "💔", desc: "登録済みファイルが見つからない" },
};

const REASON_LABELS: Record<DuplicateGroup["reason"], { label: string; icon: string }> = {
  same_tmdb_id:    { label: "同一 TMDb ID", icon: "🔗" },
  same_title_year: { label: "同名 + 製作年", icon: "🎬" },
};

// ─── タブ型 ──────────────────────────────────────────────────────────────────

type Tab = "duplicates" | "integrity";

// ─── 重複グループカード ───────────────────────────────────────────────────────

function DuplicateGroupCard({
  group,
  onNavigate,
  onDelete,
  isDeleting,
}: {
  group: DuplicateGroup;
  onNavigate: (workId: number) => void;
  onDelete: (workId: number) => void;
  isDeleting: boolean;
}) {
  const [expanded, setExpanded] = useState(false);
  const { label, icon } = REASON_LABELS[group.reason];

  return (
    <div className="border border-subtle rounded-lg overflow-hidden">
      <button
        onClick={() => setExpanded((v) => !v)}
        className="w-full flex items-center gap-3 px-4 py-3 bg-surface-elevated hover:bg-surface-hover transition-colors text-left"
      >
        <span className="text-lg">{icon}</span>
        <div className="flex-1 min-w-0">
          <span className="text-xs font-semibold text-yellow-400 mr-2">{label}</span>
          <span className="text-xs text-gray-500 font-mono">{group.key}</span>
        </div>
        <span className="text-xs text-gray-500 mr-2">{group.workIds.length} 件</span>
        <span className="text-gray-600 text-sm">{expanded ? "▲" : "▼"}</span>
      </button>

      {expanded && (
        <div className="border-t border-subtle divide-y divide-subtle">
          {group.workIds.map((id, idx) => (
            <div key={id} className="flex items-center gap-3 px-4 py-2.5 bg-surface">
              <div className="flex-1 min-w-0">
                <p className="text-xs font-medium text-gray-200 truncate">
                  {group.titles[idx] ?? "—"}
                </p>
                <p className="text-[11px] text-gray-600">
                  {group.years[idx] ?? "—"} · ID: {id}
                </p>
              </div>
              <div className="flex gap-1.5 flex-shrink-0">
                <button
                  onClick={() => onNavigate(id)}
                  className="text-xs px-2.5 py-1 border border-subtle rounded text-gray-400 hover:text-gray-100 hover:border-gray-500 transition-colors"
                >
                  詳細
                </button>
                <button
                  onClick={() => onDelete(id)}
                  disabled={isDeleting}
                  className="text-xs px-2 py-1 text-gray-700 hover:text-red-400 transition-colors disabled:opacity-30"
                  title="この作品を削除"
                >
                  ×
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ─── 整合性問題カード ─────────────────────────────────────────────────────────

function IntegrityIssueCard({
  issue,
  onNavigate,
  onFindCandidate,
  onRepairPersons,
  onRefreshMeta,
  isPending,
}: {
  issue: IntegrityIssue;
  onNavigate: (workId: number) => void;
  onFindCandidate: (issue: IntegrityIssue) => void;
  onRepairPersons: (workId: number) => void;
  onRefreshMeta: (workId: number) => void;
  isPending: boolean;
}) {
  const needsCandidate = issue.issues.includes("unmatched");
  const needsMetaRefresh = issue.issues.includes("missing_meta");
  const needsPersons = issue.issues.includes("no_persons");

  return (
    <div className="flex items-start gap-3 px-4 py-3 bg-surface-elevated border border-subtle rounded-lg">
      <div className="flex-1 min-w-0">
        <p className="text-xs font-medium text-gray-200 truncate">{issue.title}</p>
        <p className="text-[11px] text-gray-600 mt-0.5">{issue.year ?? "—"} · ID: {issue.workId}</p>
        <div className="flex flex-wrap gap-1 mt-1.5">
          {issue.issues.map((code) => {
            const { label, icon } = ISSUE_LABELS[code];
            return (
              <span
                key={code}
                className="inline-flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded bg-yellow-900/30 border border-yellow-800/50 text-yellow-500"
              >
                {icon} {label}
              </span>
            );
          })}
        </div>
      </div>

      <div className="flex flex-col gap-1 flex-shrink-0 items-end">
        <button
          onClick={() => onNavigate(issue.workId)}
          className="text-[11px] px-2.5 py-1 border border-subtle rounded text-gray-400 hover:text-gray-100 hover:border-gray-500 transition-colors"
        >
          詳細
        </button>
        {needsCandidate && (
          <button
            onClick={() => onFindCandidate(issue)}
            disabled={isPending}
            className="text-[11px] px-2.5 py-1 border border-mantis-700/50 rounded text-mantis-400 hover:bg-mantis-800/20 transition-colors disabled:opacity-30"
            title="通常画面と同じ候補選択でTMDb照合"
          >
            🔎 候補を探す
          </button>
        )}
        {needsMetaRefresh && !needsCandidate && (
          <button
            onClick={() => onRefreshMeta(issue.workId)}
            disabled={isPending}
            className="text-[11px] px-2.5 py-1 border border-mantis-700/50 rounded text-mantis-400 hover:bg-mantis-800/20 transition-colors disabled:opacity-30"
            title="この作品だけメタデータを再取得"
          >
            📋 再取得
          </button>
        )}
        {needsPersons && (
          <button
            onClick={() => onRepairPersons(issue.workId)}
            disabled={isPending}
            className="text-[11px] px-2.5 py-1 border border-blue-800/50 rounded text-blue-400 hover:bg-blue-900/20 transition-colors disabled:opacity-30"
            title="人物情報を再取得"
          >
            👤 人物再取得
          </button>
        )}
      </div>
    </div>
  );
}

// ─── メイン画面 ───────────────────────────────────────────────────────────────

export function AuditScreen() {
  const [tab, setTab] = useState<Tab>("duplicates");
  const [confirmDelete, setConfirmDelete] = useState<{ workId: number; title: string } | null>(null);
  const [candidateTarget, setCandidateTarget] = useState<IntegrityIssue | null>(null);

  const { setSelectedWorkId, setActiveSection } = useLibraryStore();

  const { data: duplicates = [], isLoading: dupLoading, refetch: refetchDup } = useDuplicateGroups();
  const { data: report, isLoading: intLoading, refetch: refetchInt } = useIntegrityReport();

  const { mutate: deleteWork, isPending: isDeleting } = useDeleteWork();
  const { mutate: repairPersons, isPending: isRepairing } = useRepairFetchPersons();
  const { mutate: refreshMeta, isPending: isRefreshing } = useRefreshTmdbMetadata();

  const isPending = isDeleting || isRepairing || isRefreshing;

  function navigateToWork(workId: number) {
    setSelectedWorkId(workId);
    setActiveSection("all-movies");
  }

  function handleDeleteConfirm(workId: number, title: string) {
    setConfirmDelete({ workId, title });
  }

  function execDelete() {
    if (!confirmDelete) return;
    deleteWork(confirmDelete.workId);
    setConfirmDelete(null);
  }

  const totalDupWorks = duplicates.reduce((sum, g) => sum + g.workIds.length, 0);

  return (
    <div className="flex-1 overflow-y-auto p-6">
      <div className="max-w-3xl mx-auto flex flex-col gap-6">

        {/* ヘッダー */}
        <div className="flex items-start justify-between">
          <div>
            <h1 className="text-lg font-semibold text-gray-100">監査レポート</h1>
            <p className="text-xs text-gray-500 mt-0.5">
              未整理・未照合・重複を確認して、候補選択や再取得で整理します
            </p>
          </div>
          <button
            onClick={() => { refetchDup(); refetchInt(); }}
            className="text-xs px-3 py-1.5 border border-subtle rounded text-gray-400 hover:text-gray-100 hover:border-gray-500 transition-colors"
          >
            🔄 再チェック
          </button>
        </div>

        {/* タブ */}
        <div className="flex gap-0 border-b border-subtle">
          {(["duplicates", "integrity"] as const).map((t) => {
            const labels: Record<Tab, string> = {
              duplicates: `重複候補 ${dupLoading ? "" : `(${duplicates.length}グループ / ${totalDupWorks}件)`}`,
              integrity:  `整合性チェック ${intLoading ? "" : `(${report?.totalIssues ?? 0}件)`}`,
            };
            return (
              <button
                key={t}
                onClick={() => setTab(t)}
                className={clsx(
                  "px-4 py-2 text-sm border-b-2 transition-colors",
                  tab === t
                    ? "border-mantis-500 text-mantis-400 font-medium"
                    : "border-transparent text-gray-500 hover:text-gray-300"
                )}
              >
                {labels[t]}
              </button>
            );
          })}
        </div>

        {/* ── 重複タブ ── */}
        {tab === "duplicates" && (
          <div className="flex flex-col gap-3">
            {dupLoading ? (
              <p className="text-sm text-gray-600 animate-pulse">スキャン中…</p>
            ) : duplicates.length === 0 ? (
              <div className="text-center py-10">
                <p className="text-3xl mb-2">✓</p>
                <p className="text-sm text-mantis-400 font-medium">重複は見つかりませんでした</p>
                <p className="text-xs text-gray-600 mt-1">すべての作品が一意です</p>
              </div>
            ) : (
              <>
                <p className="text-xs text-gray-500">
                  重複が疑われる作品グループを一覧します。不要な方を削除してください。
                </p>
                {duplicates.map((group) => (
                  <DuplicateGroupCard
                    key={`${group.reason}-${group.key}`}
                    group={group}
                    onNavigate={navigateToWork}
                    onDelete={(id) => {
                      const title = group.titles[group.workIds.indexOf(id)] ?? "";
                      handleDeleteConfirm(id, title);
                    }}
                    isDeleting={isDeleting}
                  />
                ))}
              </>
            )}
          </div>
        )}

        {/* ── 整合性タブ ── */}
        {tab === "integrity" && (
          <div className="flex flex-col gap-3">
            {intLoading ? (
              <p className="text-sm text-gray-600 animate-pulse">スキャン中…</p>
            ) : !report || report.totalIssues === 0 ? (
              <div className="text-center py-10">
                <p className="text-3xl mb-2">✓</p>
                <p className="text-sm text-mantis-400 font-medium">整合性の問題は見つかりませんでした</p>
                <p className="text-xs text-gray-600 mt-1">
                  {report?.totalWorks ?? 0} 件をスキャン — すべて正常
                </p>
              </div>
            ) : (
              <>
                {/* サマリーカード */}
                <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
                  {(Object.keys(ISSUE_LABELS) as IssueCode[]).map((code) => {
                    const { label, icon } = ISSUE_LABELS[code];
                    const count = report.issues.filter((i) => i.issues.includes(code)).length;
                    return (
                      <div
                        key={code}
                        className={clsx(
                          "flex flex-col gap-0.5 px-3 py-2.5 rounded border",
                          count > 0
                            ? "bg-yellow-900/10 border-yellow-800/40"
                            : "bg-surface border-subtle opacity-40"
                        )}
                      >
                        <span className="text-lg leading-none">{icon}</span>
                        <span className="text-xs text-gray-300 mt-1">{label}</span>
                        <span className={clsx(
                          "text-lg font-bold",
                          count > 0 ? "text-yellow-400" : "text-mantis-500"
                        )}>
                          {count}
                        </span>
                      </div>
                    );
                  })}
                </div>

                <p className="text-xs text-gray-500">
                  {report.totalWorks} 件中 {report.issues.length} 作品 / {report.totalIssues} 項目に問題があります。
                </p>

                <div className="flex flex-col gap-2">
                  {report.issues.map((issue) => (
                    <IntegrityIssueCard
                      key={issue.workId}
                      issue={issue}
                      onNavigate={navigateToWork}
                      onFindCandidate={setCandidateTarget}
                      onRepairPersons={(id) => repairPersons(id)}
                      onRefreshMeta={(id) => refreshMeta(id)}
                      isPending={isPending}
                    />
                  ))}
                </div>
              </>
            )}
          </div>
        )}
      </div>

      {/* ── 削除確認ダイアログ ── */}
      {confirmDelete && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
          <div role="dialog" className="bg-surface-elevated border border-subtle rounded-lg p-6 w-80 shadow-2xl">
            <h3 className="text-sm font-semibold text-gray-100 mb-2">作品を削除</h3>
            <p className="text-xs text-gray-400 mb-1">以下の作品をライブラリから削除します：</p>
            <p className="text-xs font-mono text-gray-300 bg-surface rounded px-2 py-1 mb-4 break-all">
              {confirmDelete.title}
            </p>
            <p className="text-xs text-red-400 mb-4">
              ⚠ 評価・視聴状態・タグなどの記録もすべて削除されます。<br />
              動画ファイル自体は削除されません。
            </p>
            <div className="flex gap-2 justify-end">
              <button
                onClick={() => setConfirmDelete(null)}
                className="px-3 py-1.5 text-xs border border-subtle rounded text-gray-400 hover:text-gray-200 transition-colors"
              >
                キャンセル
              </button>
              <button
                onClick={execDelete}
                className="px-3 py-1.5 text-xs bg-red-900/40 border border-red-700/60 rounded text-red-300 hover:bg-red-900/60 transition-colors"
              >
                削除する
              </button>
            </div>
          </div>
        </div>
      )}
      {candidateTarget && (
        <CandidateDialog
          workId={candidateTarget.workId}
          workTitle={candidateTarget.title}
          onClose={() => {
            setCandidateTarget(null);
            refetchInt();
          }}
        />
      )}
    </div>
  );
}
