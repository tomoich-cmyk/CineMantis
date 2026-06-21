import { useEffect, useMemo, useState } from "react";
import {
  addManualAwardMatchCandidate,
  approveAwardImportItem,
  bulkApproveAwardImportItems,
  bulkRejectAwardImportItems,
  listAwardImportItems,
  rejectAwardImportItem,
  searchWorksForAwardMatch,
  selectAwardMatchCandidate,
  type AwardImportItemView,
  type MatchCandidateView,
  type WorkSearchResult,
} from "@/api/awards";

type StatusFilter = "pending" | "approved" | "rejected" | "all";
type ConfidenceFilter = "all" | "high" | "review" | "low" | "unmatched";

interface ImportReviewOverlayProps {
  open: boolean;
  onClose: () => void;
  jobId: number;
  title: string;
}

const RESULT_LABELS: Record<string, string> = {
  winner: "受賞",
  nominee: "ノミネート",
  shortlisted: "候補",
  special_mention: "特別表彰",
  selection: "選出",
  unknown: "不明",
};

export function ImportReviewOverlay({ open, onClose, jobId, title }: ImportReviewOverlayProps) {
  const [items, setItems] = useState<AwardImportItemView[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [checkedIds, setCheckedIds] = useState<Set<number>>(new Set());
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("pending");
  const [confidenceFilter, setConfidenceFilter] = useState<ConfidenceFilter>("all");
  const [onlyUnmatched, setOnlyUnmatched] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [manualOpen, setManualOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    void loadItems();
  }, [open, jobId, statusFilter, onlyUnmatched]);

  async function loadItems(nextSelectedId = selectedId) {
    setLoading(true);
    setError(null);
    try {
      const rows = await listAwardImportItems({
        jobId,
        status: statusFilter === "all" ? null : statusFilter,
        onlyUnmatched,
      });
      setItems(rows);
      setSelectedId(nextSelectedId && rows.some((row) => row.id === nextSelectedId) ? nextSelectedId : rows[0]?.id ?? null);
      setCheckedIds((current) => new Set([...current].filter((id) => rows.some((row) => row.id === id))));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  const filteredItems = useMemo(() => {
    return items.filter((item) => {
      const score = item.matchScore ?? -1;
      if (confidenceFilter === "high") return score >= 90;
      if (confidenceFilter === "review") return score >= 70 && score < 90;
      if (confidenceFilter === "low") return score >= 0 && score < 70;
      if (confidenceFilter === "unmatched") return item.matchedWorkId === null;
      return true;
    });
  }, [items, confidenceFilter]);

  const selectedItem = filteredItems.find((item) => item.id === selectedId) ?? filteredItems[0] ?? null;
  const highConfidenceIds = filteredItems
    .filter((item) => item.status === "pending" && (item.matchScore ?? 0) >= 90 && item.matchedWorkId)
    .map((item) => item.id);

  async function approveOne(item: AwardImportItemView) {
    setError(null);
    try {
      await approveAwardImportItem(item.id);
      setItems((current) => current.map((row) => (row.id === item.id ? { ...row, status: "approved" } : row)));
      await loadItems(item.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function rejectOne(item: AwardImportItemView) {
    setError(null);
    try {
      await rejectAwardImportItem(item.id);
      setItems((current) => current.map((row) => (row.id === item.id ? { ...row, status: "rejected" } : row)));
      await loadItems(item.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function selectCandidate(item: AwardImportItemView, candidate: MatchCandidateView) {
    setError(null);
    try {
      await selectAwardMatchCandidate(item.id, candidate.id);
      await loadItems(item.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function bulkApprove(ids: number[], label: string) {
    if (ids.length === 0) return;
    if (!window.confirm(`${label} ${ids.length}件を承認します。よろしいですか？`)) return;
    setError(null);
    const result = await bulkApproveAwardImportItems(ids);
    if (result.errors.length > 0) setError(result.errors.join("\n"));
    await loadItems();
  }

  async function bulkReject(ids: number[]) {
    if (ids.length === 0) return;
    if (!window.confirm(`選択した ${ids.length}件を却下します。よろしいですか？`)) return;
    setError(null);
    const result = await bulkRejectAwardImportItems(ids);
    if (result.errors.length > 0) setError(result.errors.join("\n"));
    await loadItems();
  }

  function toggleChecked(id: number) {
    setCheckedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex flex-col bg-[#090b0d] text-gray-200">
      <header className="flex h-12 items-center justify-between border-b border-subtle bg-surface-elevated px-4">
        <div className="min-w-0">
          <h2 className="truncate text-sm font-semibold">候補確認 - {title}</h2>
          <p className="text-[11px] text-gray-600">
            {filteredItems.length}件表示 / {items.length}件
          </p>
        </div>
        <button onClick={onClose} className="px-3 py-1 text-sm text-gray-400 hover:text-gray-100">
          閉じる
        </button>
      </header>

      <div className="flex h-10 items-center gap-2 border-b border-subtle bg-black px-3 text-xs">
        <select
          value={statusFilter}
          onChange={(event) => setStatusFilter(event.target.value as StatusFilter)}
          className="h-7 border border-subtle bg-surface-elevated px-2 text-gray-200"
        >
          <option value="pending">未確認</option>
          <option value="approved">承認済み</option>
          <option value="rejected">却下済み</option>
          <option value="all">すべて</option>
        </select>
        <select
          value={confidenceFilter}
          onChange={(event) => setConfidenceFilter(event.target.value as ConfidenceFilter)}
          className="h-7 border border-subtle bg-surface-elevated px-2 text-gray-200"
        >
          <option value="all">信頼度すべて</option>
          <option value="high">高信頼</option>
          <option value="review">要確認</option>
          <option value="low">低信頼</option>
          <option value="unmatched">未照合</option>
        </select>
        <label className="flex items-center gap-1 text-gray-400">
          <input type="checkbox" checked={onlyUnmatched} onChange={(event) => setOnlyUnmatched(event.target.checked)} />
          未照合/低信頼のみ
        </label>
        <div className="ml-auto flex items-center gap-2">
          <button
            onClick={() => void bulkApprove(highConfidenceIds, "高信頼")}
            className="border border-mantis-700 px-3 py-1 text-mantis-200 hover:bg-mantis-900/40"
          >
            高信頼を承認 ({highConfidenceIds.length})
          </button>
          <button
            onClick={() => void bulkApprove([...checkedIds], "選択")}
            className="border border-subtle px-3 py-1 hover:border-mantis-700"
          >
            選択を承認 ({checkedIds.size})
          </button>
          <button
            onClick={() => void bulkReject([...checkedIds])}
            className="border border-subtle px-3 py-1 text-red-300 hover:border-red-800"
          >
            選択を却下
          </button>
        </div>
      </div>

      {error && <pre className="max-h-24 overflow-auto border-b border-red-900 bg-red-950/30 p-2 text-xs text-red-300">{error}</pre>}

      <div className="grid min-h-0 flex-1 grid-cols-[1fr_360px]">
        <div className="min-w-0 overflow-auto">
          {loading ? (
            <div className="p-5 text-sm text-gray-500">読み込み中...</div>
          ) : (
            <>
              <table className="w-full table-fixed border-collapse text-xs">
                <thead className="sticky top-0 z-10 bg-surface-elevated text-left text-gray-500">
                  <tr>
                    <th className="w-9 border-b border-subtle px-2 py-2"></th>
                    <th className="w-20 border-b border-subtle px-2 py-2">状態</th>
                    <th className="w-16 border-b border-subtle px-2 py-2">年度</th>
                    <th className="border-b border-subtle px-2 py-2">Wikidataタイトル</th>
                    <th className="border-b border-subtle px-2 py-2">ローカル候補</th>
                    <th className="w-20 border-b border-subtle px-2 py-2 text-right">スコア</th>
                    <th className="w-24 border-b border-subtle px-2 py-2">結果</th>
                  </tr>
                </thead>
                <tbody>
                  {filteredItems.map((item) => (
                    <tr
                      key={item.id}
                      onClick={() => setSelectedId(item.id)}
                      className={`h-8 cursor-pointer border-b border-subtle/60 hover:bg-surface-hover ${
                        selectedItem?.id === item.id ? "bg-mantis-950/40 outline outline-1 outline-mantis-800" : ""
                      } ${item.status === "approved" ? "opacity-60" : ""} ${item.status === "rejected" ? "opacity-40 line-through" : ""}`}
                    >
                      <td className="px-2">
                        <input
                          type="checkbox"
                          checked={checkedIds.has(item.id)}
                          onChange={() => toggleChecked(item.id)}
                          onClick={(event) => event.stopPropagation()}
                        />
                      </td>
                      <td className="px-2">{statusLabel(item)}</td>
                      <td className="px-2 text-gray-400">{item.rawYear ?? "-"}</td>
                      <td className="truncate px-2 font-medium text-gray-200">{displayImportTitle(item)}</td>
                      <td className="truncate px-2 text-gray-400">
                        {item.matchedWorkTitle ? `${item.matchedWorkTitle}${item.matchedWorkYear ? ` (${item.matchedWorkYear})` : ""}` : "-"}
                      </td>
                      <td className={`px-2 text-right ${scoreClass(item.matchScore)}`}>{scoreLabel(item.matchScore)}</td>
                      <td className="px-2 text-gray-500">{RESULT_LABELS[item.rawResultType] ?? item.rawResultType}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {filteredItems.length === 0 && (
                <div className="p-6 text-sm text-gray-500">
                  この条件で表示できる候補がありません。状態や信頼度フィルタを変えるか、もう一度取得して照合してください。
                </div>
              )}
            </>
          )}
        </div>

        <aside className="min-h-0 overflow-auto border-l border-subtle bg-surface-elevated p-4">
          {selectedItem ? (
            <ImportItemDetail
              item={selectedItem}
              onApprove={() => void approveOne(selectedItem)}
              onReject={() => void rejectOne(selectedItem)}
              onSelectCandidate={(candidate) => void selectCandidate(selectedItem, candidate)}
              onOpenManual={() => setManualOpen(true)}
            />
          ) : (
            <div className="text-sm text-gray-600">候補を選択してください</div>
          )}
        </aside>
      </div>

      {selectedItem && manualOpen && (
        <ManualSearchModal
          item={selectedItem}
          onClose={() => setManualOpen(false)}
          onApplied={async (candidateId) => {
            await selectAwardMatchCandidate(selectedItem.id, candidateId);
            setManualOpen(false);
            await loadItems(selectedItem.id);
          }}
        />
      )}
    </div>
  );
}

function ImportItemDetail({
  item,
  onApprove,
  onReject,
  onSelectCandidate,
  onOpenManual,
}: {
  item: AwardImportItemView;
  onApprove: () => void;
  onReject: () => void;
  onSelectCandidate: (candidate: MatchCandidateView) => void;
  onOpenManual: () => void;
}) {
  return (
    <div className="space-y-4 text-xs">
      <section>
        <h3 className="text-sm font-semibold text-gray-100">{displayImportTitle(item)}</h3>
        <div className="mt-1 text-gray-500">
          {item.rawYear ?? "-"} ・ {RESULT_LABELS[item.rawResultType] ?? item.rawResultType}
        </div>
        {item.rawTitleEn && <div className="mt-1 italic text-gray-500">{item.rawTitleEn}</div>}
        {item.rawSourceUrl && (
          <a className="mt-2 inline-block text-mantis-300 hover:underline" href={item.rawSourceUrl} target="_blank" rel="noreferrer">
            Wikidataを開く
          </a>
        )}
      </section>

      <section>
        <div className="mb-2 flex items-center justify-between">
          <h4 className="font-medium text-gray-300">候補</h4>
          <button onClick={onOpenManual} className="border border-subtle px-2 py-1 text-gray-300 hover:border-mantis-700">
            手動検索
          </button>
        </div>
        <div className="space-y-2">
          {item.candidates.length === 0 && <div className="text-gray-600">候補がありません</div>}
          {item.candidates.map((candidate) => (
            <button
              key={candidate.id}
              onClick={() => onSelectCandidate(candidate)}
              className={`w-full border p-2 text-left hover:border-mantis-700 ${
                candidate.isSelected ? "border-mantis-700 bg-mantis-950/40" : "border-subtle bg-black/20"
              }`}
            >
              <div className="flex items-center justify-between gap-2">
                <span className="truncate font-medium text-gray-200">
                  {candidate.workTitle}
                  {candidate.workYear ? ` (${candidate.workYear})` : ""}
                </span>
                <span className={scoreClass(candidate.score)}>{scoreLabel(candidate.score)}</span>
              </div>
              <div className="mt-1 truncate text-gray-600">{candidate.workSourcePath ?? "-"}</div>
              <div className="mt-1 text-gray-500">{candidate.matchMethod}</div>
            </button>
          ))}
        </div>
      </section>

      <section className="grid grid-cols-2 gap-2">
        <button
          onClick={onApprove}
          disabled={!item.matchedWorkId || item.status === "approved"}
          className="border border-mantis-700 bg-mantis-900/40 px-3 py-2 text-mantis-200 hover:bg-mantis-800/50 disabled:cursor-not-allowed disabled:opacity-40"
        >
          承認
        </button>
        <button
          onClick={onReject}
          disabled={item.status === "rejected"}
          className="border border-red-900 px-3 py-2 text-red-300 hover:bg-red-950/30 disabled:cursor-not-allowed disabled:opacity-40"
        >
          却下
        </button>
      </section>
    </div>
  );
}

function ManualSearchModal({
  item,
  onClose,
  onApplied,
}: {
  item: AwardImportItemView;
  onClose: () => void;
  onApplied: (candidateId: number) => Promise<void>;
}) {
  const [query, setQuery] = useState(displayImportTitle(item));
  const [year, setYear] = useState(item.rawYear?.toString() ?? "");
  const [results, setResults] = useState<WorkSearchResult[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function search() {
    setLoading(true);
    setError(null);
    try {
      const parsedYear = year.trim() ? Number(year.trim()) : null;
      setResults(await searchWorksForAwardMatch(query, Number.isFinite(parsedYear) ? parsedYear : null));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  async function apply(work: WorkSearchResult) {
    setError(null);
    try {
      const candidateId = await addManualAwardMatchCandidate(item.id, work.id);
      await onApplied(candidateId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/70">
      <div className="w-[680px] max-w-[calc(100vw-32px)] border border-subtle bg-surface-elevated p-4 shadow-xl">
        <div className="mb-3 flex items-center justify-between">
          <h3 className="text-sm font-semibold text-gray-100">作品を手動検索</h3>
          <button onClick={onClose} className="text-gray-500 hover:text-gray-100">閉じる</button>
        </div>
        <div className="flex gap-2">
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            className="h-8 flex-1 border border-subtle bg-black px-2 text-sm text-gray-200"
          />
          <input
            value={year}
            onChange={(event) => setYear(event.target.value.replace(/[^\d]/g, "").slice(0, 4))}
            className="h-8 w-24 border border-subtle bg-black px-2 text-sm text-gray-200"
          />
          <button onClick={() => void search()} className="h-8 border border-mantis-700 px-4 text-sm text-mantis-200">
            検索
          </button>
        </div>
        {error && <div className="mt-2 text-xs text-red-400">{error}</div>}
        <div className="mt-3 max-h-[420px] overflow-auto border border-subtle">
          {loading && <div className="p-3 text-sm text-gray-500">検索中...</div>}
          {!loading && results.map((work) => (
            <button
              key={work.id}
              onClick={() => void apply(work)}
              className="block w-full border-b border-subtle/70 p-3 text-left text-sm hover:bg-surface-hover"
            >
              <div className="font-medium text-gray-200">
                {work.title}
                {work.year ? ` (${work.year})` : ""}
              </div>
              <div className="mt-1 truncate text-xs text-gray-600">{work.sourcePath ?? "-"}</div>
              <div className="mt-1 text-xs text-gray-500">
                TMDb {work.tmdbId ?? "-"} / IMDb {work.imdbId ?? "-"}
              </div>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

function displayImportTitle(item: AwardImportItemView) {
  return item.rawTitleJa || item.rawTitleEn || item.rawAwardNameJa || item.rawAwardNameEn || `import #${item.id}`;
}

function scoreLabel(score: number | null) {
  if (score === null || score < 0) return "-";
  return Math.round(score).toString();
}

function scoreClass(score: number | null) {
  if (score === null || score < 0) return "text-gray-600";
  if (score >= 90) return "text-mantis-300";
  if (score >= 70) return "text-yellow-300";
  return "text-red-300";
}

function statusLabel(item: AwardImportItemView) {
  if (item.alreadyConfirmed) return <span className="text-mantis-300">登録済</span>;
  if (item.status === "approved") return <span className="text-mantis-300">承認済</span>;
  if (item.status === "rejected") return <span className="text-red-300">却下</span>;
  if (!item.matchedWorkId) return <span className="text-red-300">未照合</span>;
  if ((item.matchScore ?? 0) >= 90) return <span className="text-mantis-300">高信頼</span>;
  if ((item.matchScore ?? 0) >= 70) return <span className="text-yellow-300">要確認</span>;
  return <span className="text-red-300">低信頼</span>;
}
