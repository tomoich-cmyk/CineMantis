import { clsx } from "clsx";
import { useLibraryStore } from "@/store/libraryStore";
import { useFilterOptions } from "@/hooks/useWorks";
import { usePersonsList } from "@/hooks/usePersons";
import { useSeriesList } from "@/hooks/useSeries";

export function useActiveFilterCount(): number {
  const { filters } = useLibraryStore();
  let count = 0;
  if (filters.query.trim()) count++;
  if (filters.workType !== null) count++;
  if (filters.yearFrom !== null) count++;
  if (filters.yearTo !== null) count++;
  if (filters.genre !== null) count++;
  if (filters.country !== null) count++;
  if (filters.countryType !== null) count++;
  if (filters.personId !== null) count++;
  if (filters.seriesId !== null) count++;
  if (filters.minUserRating !== null) count++;
  if (filters.unorganizedOnly) count++;
  return count;
}

const COUNTRY_TYPE_OPTIONS = [
  ["foreign", "洋画"],
  ["domestic", "邦画"],
  ["unknown", "不明"],
] as const;

function fieldClass(extra = "") {
  return `bg-surface-elevated border border-subtle rounded px-2 py-0.5 text-xs text-gray-300 outline-none focus:border-mantis-600 ${extra}`;
}

export function FilterBar() {
  const { filters, setFilter, resetFilters } = useLibraryStore();
  const { data: opts } = useFilterOptions();
  const { data: allPersons = [] } = usePersonsList();
  const { data: allSeries = [] } = useSeriesList();
  const activeCount = useActiveFilterCount();

  return (
    <div className="px-4 py-3 border-b border-subtle bg-surface flex flex-col gap-3">
      <div className="flex items-center gap-3 flex-wrap">
        <input
          value={filters.query}
          onChange={(e) => setFilter("query", e.target.value)}
          placeholder="検索"
          className={fieldClass("w-56")}
        />

        <select
          value={filters.countryType ?? ""}
          onChange={(e) => setFilter("countryType", e.target.value || null)}
          className={fieldClass("min-w-[80px]")}
        >
          <option value="">洋邦</option>
          {COUNTRY_TYPE_OPTIONS.map(([value, label]) => (
            <option key={value} value={value}>{label}</option>
          ))}
        </select>

        <select
          value={filters.genre ?? ""}
          onChange={(e) => setFilter("genre", e.target.value || null)}
          className={fieldClass("min-w-[120px]")}
        >
          <option value="">ジャンル</option>
          {opts?.genres.map((g) => (
            <option key={g} value={g}>{g}</option>
          ))}
        </select>

      </div>

      <div className="flex items-center gap-3 flex-wrap">
        <label className="flex items-center gap-1 text-xs text-gray-500">
          評価
          <input
            type="number"
            min={0}
            max={10}
            step={0.1}
            value={filters.minUserRating ?? ""}
            onChange={(e) => {
              const raw = e.target.value;
              const next = raw === "" ? null : Math.max(0, Math.min(10, Math.round(Number(raw) * 10) / 10));
              setFilter("minUserRating", Number.isFinite(next) ? next : null);
            }}
            placeholder="0.0"
            className={fieldClass("w-16 text-right font-mono")}
          />
          <span className="text-gray-700">以上</span>
        </label>

        <select
          value={filters.personId ?? ""}
          onChange={(e) => setFilter("personId", e.target.value ? Number(e.target.value) : null)}
          className={fieldClass("min-w-[130px]")}
        >
          <option value="">人物</option>
          {allPersons.map((p) => (
            <option key={p.id} value={p.id}>{p.name}</option>
          ))}
        </select>

        <select
          value={filters.seriesId ?? ""}
          onChange={(e) => setFilter("seriesId", e.target.value ? Number(e.target.value) : null)}
          className={fieldClass("min-w-[130px]")}
        >
          <option value="">シリーズ</option>
          {allSeries.map((s) => (
            <option key={s.id} value={s.id}>{s.title}</option>
          ))}
        </select>

        <button
          onClick={() => setFilter("unorganizedOnly", !filters.unorganizedOnly)}
          className={clsx(
            "px-2.5 py-0.5 text-xs rounded border transition-colors",
            filters.unorganizedOnly
              ? "bg-orange-900/30 border-orange-700/60 text-orange-300"
              : "border-subtle text-gray-500 hover:text-gray-300",
          )}
        >
          未整理
        </button>

        <div className="flex-1" />
        {activeCount > 0 && (
          <button
            onClick={resetFilters}
            className="text-xs text-gray-600 hover:text-gray-300 underline-offset-2 hover:underline"
          >
            フィルタ解除
          </button>
        )}
      </div>
    </div>
  );
}
