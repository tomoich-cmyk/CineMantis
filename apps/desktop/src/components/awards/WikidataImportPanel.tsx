import { useEffect, useState } from "react";
import type { AwardBody, AwardCategory } from "@cinemantis/shared-types";
import {
  fetchWikidataAwardItems,
  matchAwardImportItems,
  type AwardImportJobSummary,
  type AwardImportMatchSummary,
} from "@/api/awards";
import { ImportReviewOverlay } from "@/components/awards/ImportReviewOverlay";

interface WikidataImportPanelProps {
  awardBody: AwardBody;
  categories: AwardCategory[];
  categoryId: number | null;
  onCategoryChange: (categoryId: number | null) => void;
}

export function WikidataImportPanel({
  awardBody,
  categories,
  categoryId,
  onCategoryChange,
}: WikidataImportPanelProps) {
  const [year, setYear] = useState("");
  const [job, setJob] = useState<AwardImportJobSummary | null>(null);
  const [matchSummary, setMatchSummary] = useState<AwardImportMatchSummary | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reviewOpen, setReviewOpen] = useState(false);

  useEffect(() => {
    setJob(null);
    setMatchSummary(null);
    setError(null);
    setReviewOpen(false);
  }, [awardBody.id, categoryId]);

  const importableCategories = categories.filter((category) => category.wikidataEntityId);
  const hiddenCategoryCount = categories.length - importableCategories.length;
  const selectedCategory =
    importableCategories.find((category) => category.id === categoryId) ?? null;

  useEffect(() => {
    if (categoryId !== null && importableCategories.length > 0 && !selectedCategory) {
      onCategoryChange(null);
    }
  }, [categoryId, importableCategories.length, onCategoryChange, selectedCategory]);

  async function runImport() {
    setIsRunning(true);
    setError(null);
    setMatchSummary(null);
    try {
      const parsedYear = year.trim() ? Number(year.trim()) : null;
      const fetched = await fetchWikidataAwardItems(
        awardBody.id,
        categoryId,
        Number.isFinite(parsedYear) ? parsedYear : null,
      );
      setJob(fetched);
      if (fetched.status === "error") {
        setError(fetched.errorMessage ?? "Wikidataからの取得に失敗しました");
        return;
      }
      const summary = await matchAwardImportItems(fetched.jobId);
      setMatchSummary(summary);
      setReviewOpen(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setIsRunning(false);
    }
  }

  return (
    <section className="border-b border-subtle bg-black/20 px-5 py-3">
      <div className="flex flex-wrap items-end gap-3">
        <div className="min-w-[280px] flex-1">
          <label className="mb-1 block text-[11px] text-gray-500">Wikidata 取込カテゴリ</label>
          <select
            value={categoryId ?? ""}
            onChange={(event) => {
              onCategoryChange(event.target.value ? Number(event.target.value) : null);
            }}
            className="h-8 w-full border border-subtle bg-surface-elevated px-2 text-sm text-gray-200 outline-none focus:border-mantis-600"
          >
            <option value="">賞全体から取得</option>
            {importableCategories.map((category) => (
              <option key={category.id} value={category.id}>
                {category.displayNameJa} / {category.name}
              </option>
            ))}
          </select>
          {hiddenCategoryCount > 0 && (
            <div className="mt-1 text-[11px] text-gray-600">
              Wikidata ID未設定の{hiddenCategoryCount}カテゴリは取込対象から除外しています
            </div>
          )}
        </div>
        <div className="w-28">
          <label className="mb-1 block text-[11px] text-gray-500">年度</label>
          <input
            value={year}
            onChange={(event) => setYear(event.target.value.replace(/[^\d]/g, "").slice(0, 4))}
            placeholder="全年度"
            className="h-8 w-full border border-subtle bg-surface-elevated px-2 text-sm text-gray-200 outline-none focus:border-mantis-600"
          />
        </div>
        <button
          onClick={runImport}
          disabled={isRunning || !awardBody.wikidataEntityId}
          className="h-8 border border-mantis-700 bg-mantis-900/50 px-4 text-sm font-medium text-mantis-200 hover:bg-mantis-800/60 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {isRunning ? "取得中..." : "取得して照合"}
        </button>
        {job && (
          <button
            onClick={() => setReviewOpen(true)}
            className="h-8 border border-subtle px-4 text-sm text-gray-300 hover:border-mantis-700 hover:text-mantis-200"
          >
            候補を確認
          </button>
        )}
      </div>

      <div className="mt-2 flex flex-wrap items-center gap-4 text-xs text-gray-500">
        <span>対象: {selectedCategory ? selectedCategory.displayNameJa : "賞全体"}</span>
        {job && <span>取得 {job.insertedItems}件 / スキップ {job.skippedItems}件</span>}
        {matchSummary && (
          <span>
            高信頼 {matchSummary.highConfidence} / 要確認 {matchSummary.needsReview} / 未照合{" "}
            {matchSummary.unmatched}
          </span>
        )}
        {!awardBody.wikidataEntityId && <span className="text-yellow-400">Wikidata ID 未設定</span>}
        {error && <span className="text-red-400">{error}</span>}
      </div>

      {job && (
        <ImportReviewOverlay
          open={reviewOpen}
          onClose={() => setReviewOpen(false)}
          jobId={job.jobId}
          title={`${awardBody.displayNameJa} / ${selectedCategory?.displayNameJa ?? "賞全体"}`}
        />
      )}
    </section>
  );
}
