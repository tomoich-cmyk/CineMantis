import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useLibraryStore, DEFAULT_COLUMN_WIDTHS, DEFAULT_VISIBLE_COLUMNS } from "@/store/libraryStore";
import type { Density } from "@/store/libraryStore";
import { useWorkList } from "@/hooks/useWorks";
import { WorkCard } from "./WorkCard";
import { WorkRow } from "./WorkRow";
import { BulkActionBar } from "./BulkActionBar";
import type { WorkSummary, SortField } from "@cinemantis/shared-types";

const GRID_MIN_WIDTH: Record<Density, string> = {
  compact: "110px",
  normal: "140px",
  relaxed: "180px",
};

const ROW_HEIGHT: Record<Density, number> = {
  compact: 28,
  normal: 32,
  relaxed: 40,
};

export const COLUMN_DEFS: { key: string; label: string; sort?: SortField; min: number }[] = [
  { key: "title", label: "タイトル", sort: "title", min: 160 },
  { key: "releaseYear", label: "年", sort: "release_year", min: 56 },
  { key: "mediaCategory", label: "種別", sort: "media_category", min: 70 },
  { key: "countryType", label: "洋邦", sort: "country_type", min: 66 },
  { key: "genreText", label: "ジャンル", min: 110 },
  { key: "myRating", label: "評価", sort: "my_rating", min: 92 },
  { key: "watchedStatus", label: "視聴状態", sort: "watched_status", min: 86 },
  { key: "dateAdded", label: "登録日時", sort: "date_added", min: 112 },
  { key: "lastWatchedAt", label: "最終視聴", sort: "last_watched_at", min: 112 },
  { key: "storagePath", label: "保存場所", min: 180 },
];

const CATEGORY_LABEL: Record<string, string> = {
  movie: "映画",
  drama: "ドラマ",
  ova: "OVA",
  other: "その他",
};

const COUNTRY_LABEL: Record<string, string> = {
  foreign: "洋画",
  domestic: "邦画",
  unknown: "不明",
};

function decadeOf(work: WorkSummary) {
  const year = work.releaseYear ?? work.year;
  if (!year) return "不明";
  return `${Math.floor(year / 10) * 10}s`;
}

function ratingBucket(work: WorkSummary) {
  const rating = work.myRating ?? work.userRating;
  if (!rating) return "未評価";
  return `★ ${rating}`;
}

function splitGenres(value: string | null) {
  if (!value) return ["未設定"];
  return value
    .replace(/^\[|\]$/g, "")
    .split(/[,\u3001/]/)
    .map((v) => v.replace(/^"|"$/g, "").trim())
    .filter(Boolean)
    .slice(0, 6);
}

function makeCounts(works: WorkSummary[], getValues: (work: WorkSummary) => string[]) {
  const counts = new Map<string, number>();
  for (const work of works) {
    for (const value of getValues(work)) {
      counts.set(value, (counts.get(value) ?? 0) + 1);
    }
  }
  return Array.from(counts.entries()).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0], "ja"));
}

function BrowserPane({
  title,
  items,
  active,
  onPick,
}: {
  title: string;
  items: [string, number][];
  active: string | null;
  onPick: (value: string | null) => void;
}) {
  return (
    <section className="min-w-0 flex flex-col border-r border-[#161616] bg-[#090909]">
      <div className="h-7 flex items-center px-3 text-xs text-gray-500 border-b border-[#161616]">{title}</div>
      <div className="flex-1 overflow-auto">
        <button
          onClick={() => onPick(null)}
          className={`w-full grid grid-cols-[1fr_auto] gap-2 px-3 py-1 text-left text-xs ${active === null ? "bg-mantis-600 text-black" : "text-gray-300 hover:bg-[#141414]"}`}
        >
          <span className="truncate">すべて</span>
          <span>{items.reduce((sum, [, count]) => sum + count, 0).toLocaleString("ja-JP")}</span>
        </button>
        {items.map(([label, count]) => (
          <button
            key={label}
            onClick={() => onPick(label)}
            className={`w-full grid grid-cols-[1fr_auto] gap-2 px-3 py-1 text-left text-xs ${active === label ? "bg-mantis-600 text-black" : "text-gray-400 hover:bg-[#141414] hover:text-gray-100"}`}
          >
            <span className="truncate">{label}</span>
            <span>{count.toLocaleString("ja-JP")}</span>
          </button>
        ))}
      </div>
    </section>
  );
}

function LibraryBrowser({ works }: { works: WorkSummary[] }) {
  const { filters, setFilter } = useLibraryStore();
  const categoryCounts = useMemo(
    () => makeCounts(works, (work) => [CATEGORY_LABEL[work.mediaCategory] ?? work.mediaCategory ?? "その他"]),
    [works],
  );
  const countryCounts = useMemo(
    () => makeCounts(works, (work) => [COUNTRY_LABEL[work.countryType] ?? work.countryType ?? "不明"]),
    [works],
  );
  const decadeCounts = useMemo(() => makeCounts(works, (work) => [decadeOf(work)]), [works]);
  const ratingCounts = useMemo(() => makeCounts(works, (work) => [ratingBucket(work)]), [works]);
  const genreCounts = useMemo(() => makeCounts(works, (work) => splitGenres(work.genreText)), [works]);

  const categoryReverse: Record<string, string> = { 映画: "movie", ドラマ: "drama", OVA: "ova", その他: "other" };
  const countryReverse: Record<string, string> = { 洋画: "foreign", 邦画: "domestic", 不明: "unknown" };

  return (
    <div className="h-48 grid grid-cols-[1.1fr_0.9fr_0.9fr_0.8fr_1.2fr] border-b border-[#161616] bg-black">
      <BrowserPane
        title="種別"
        items={categoryCounts}
        active={filters.mediaCategory ? CATEGORY_LABEL[filters.mediaCategory] : null}
        onPick={(value) => setFilter("mediaCategory", value ? categoryReverse[value] ?? null : null)}
      />
      <BrowserPane
        title="洋邦"
        items={countryCounts}
        active={filters.countryType ? COUNTRY_LABEL[filters.countryType] : null}
        onPick={(value) => setFilter("countryType", value ? countryReverse[value] ?? null : null)}
      />
      <BrowserPane
        title="年代"
        items={decadeCounts}
        active={filters.yearFrom !== null && filters.yearTo !== null ? `${Math.floor(filters.yearFrom / 10) * 10}s` : null}
        onPick={(value) => {
          if (!value || value === "不明") {
            setFilter("yearFrom", null);
            setFilter("yearTo", null);
            return;
          }
          const start = Number(value.slice(0, 4));
          setFilter("yearFrom", start);
          setFilter("yearTo", start + 9);
        }}
      />
      <BrowserPane
        title="評価"
        items={ratingCounts}
        active={filters.minUserRating ? `★ ${filters.minUserRating}` : null}
        onPick={(value) => setFilter("minUserRating", value?.startsWith("★") ? Number(value.replace("★", "").trim()) : null)}
      />
      <BrowserPane
        title="ジャンル"
        items={genreCounts}
        active={filters.genre}
        onPick={(value) => setFilter("genre", value === "未設定" ? null : value)}
      />
    </div>
  );
}

function VirtualList({
  works,
  selectedWorkId,
  setSelectedWorkId,
  density,
  isSelectMode,
  selectedWorkIds,
  selectAllWorks,
  clearSelection,
}: {
  works: WorkSummary[];
  selectedWorkId: number | null;
  setSelectedWorkId: (id: number | null) => void;
  density: Density;
  isSelectMode: boolean;
  selectedWorkIds: number[];
  selectAllWorks: (ids: number[]) => void;
  clearSelection: () => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const resizeRef = useRef<{ key: string; startX: number; startWidth: number } | null>(null);
  const [columnMenuOpen, setColumnMenuOpen] = useState(false);
  const {
    sortField,
    sortOrder,
    setSortField,
    setSortOrder,
    visibleColumns,
    setVisibleColumns,
    columnWidths,
    setColumnWidth,
  } = useLibraryStore();

  const visibleDefs = COLUMN_DEFS.filter((c) => visibleColumns.includes(c.key));
  const gridTemplateColumns = [
    ...(isSelectMode ? ["36px"] : []),
    ...visibleDefs.map((column) => `${columnWidths[column.key] ?? DEFAULT_COLUMN_WIDTHS[column.key] ?? 120}px`),
  ].join(" ");

  const virtualizer = useVirtualizer({
    count: works.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT[density],
    overscan: 12,
    getItemKey: (i) => works[i].id,
  });

  useEffect(() => {
    if (selectedWorkId === null) return;
    const idx = works.findIndex((w) => w.id === selectedWorkId);
    if (idx !== -1) virtualizer.scrollToIndex(idx, { align: "auto", behavior: "smooth" });
  }, [selectedWorkId]);

  useEffect(() => {
    function onMove(event: MouseEvent) {
      const current = resizeRef.current;
      if (!current) return;
      setColumnWidth(current.key, current.startWidth + event.clientX - current.startX);
    }
    function onUp() {
      resizeRef.current = null;
      document.body.classList.remove("cm-resizing");
    }
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [setColumnWidth]);

  const items = virtualizer.getVirtualItems();
  const allIds = works.map((w) => w.id);
  const allSelected = allIds.length > 0 && allIds.every((id) => selectedWorkIds.includes(id));

  function toggleSort(sort: SortField | undefined) {
    if (!sort) return;
    if (sortField === sort) {
      setSortOrder(sortOrder === "asc" ? "desc" : "asc");
    } else {
      setSortField(sort);
      setSortOrder("asc");
    }
  }

  function toggleColumn(key: string) {
    const next = visibleColumns.includes(key)
      ? visibleColumns.filter((c) => c !== key)
      : [...visibleColumns, key];
    setVisibleColumns(next.length > 0 ? next : DEFAULT_VISIBLE_COLUMNS);
  }

  function startResize(event: React.MouseEvent, key: string) {
    event.preventDefault();
    event.stopPropagation();
    resizeRef.current = {
      key,
      startX: event.clientX,
      startWidth: columnWidths[key] ?? DEFAULT_COLUMN_WIDTHS[key] ?? 120,
    };
    document.body.classList.add("cm-resizing");
  }

  return (
    <div className="flex-1 flex flex-col overflow-hidden bg-[#070707]">
      <div className="h-8 flex items-center justify-between px-3 border-b border-[#161616] bg-[#0b0b0b]">
        <div className="text-xs text-gray-500">トラック: {works.length.toLocaleString("ja-JP")} 件</div>
        <div className="relative">
          <button
            onClick={() => setColumnMenuOpen((v) => !v)}
            className="px-2 py-0.5 text-xs border border-[#2a2a2a] text-gray-400 hover:text-gray-100"
          >
            表示列
          </button>
          {columnMenuOpen && (
            <div className="absolute right-0 mt-1 w-44 border border-[#2a2a2a] bg-[#0c0c0c] shadow-xl z-30 p-1">
              {COLUMN_DEFS.map((column) => (
                <label key={column.key} className="flex items-center gap-2 px-2 py-1 text-xs text-gray-400 hover:bg-[#181818]">
                  <input
                    type="checkbox"
                    checked={visibleColumns.includes(column.key)}
                    onChange={() => toggleColumn(column.key)}
                  />
                  {column.label}
                </label>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="overflow-auto" ref={scrollRef}>
        <div className="min-w-max">
          <div
            className="sticky top-0 z-20 grid h-8 border-b border-[#202020] bg-[#0d0d0d] text-xs text-gray-500"
            style={{ gridTemplateColumns }}
          >
            {isSelectMode && (
              <button
                onClick={() => (allSelected ? clearSelection() : selectAllWorks(allIds))}
                className="flex items-center justify-center border-r border-[#1b1b1b]"
                title={allSelected ? "全解除" : "全選択"}
              >
                {allSelected ? "✓" : ""}
              </button>
            )}
            {visibleDefs.map((column) => (
              <div key={column.key} className="relative flex items-center border-r border-[#1b1b1b]">
                <button
                  type="button"
                  onClick={() => toggleSort(column.sort)}
                  className={`w-full h-full px-2 text-left truncate ${column.sort ? "hover:text-gray-200" : "cursor-default"}`}
                >
                  {column.label}
                  {column.sort && sortField === column.sort && (
                    <span className="ml-1 text-mantis-400">{sortOrder === "asc" ? "▲" : "▼"}</span>
                  )}
                </button>
                <button
                  type="button"
                  aria-label={`${column.label} の列幅を変更`}
                  onMouseDown={(event) => startResize(event, column.key)}
                  className="absolute right-0 top-0 h-full w-1.5 cursor-col-resize hover:bg-mantis-500/70"
                />
              </div>
            ))}
          </div>

          <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {items.map((vItem) => {
              const work = works[vItem.index];
              return (
                <WorkRow
                  key={work.id}
                  work={work}
                  selected={selectedWorkId === work.id}
                  onSelect={() => setSelectedWorkId(selectedWorkId === work.id ? null : work.id)}
                  gridTemplateColumns={gridTemplateColumns}
                  top={vItem.start}
                  height={vItem.size}
                />
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}

export function WorkList() {
  const {
    viewMode,
    density,
    selectedWorkId,
    setSelectedWorkId,
    isSelectMode,
    selectedWorkIds,
    selectAllWorks,
    clearSelection,
  } = useLibraryStore();
  const { data: works = [], isLoading, isError, error } = useWorkList();

  if (isLoading) {
    return (
      <div className="flex-1 flex items-center justify-center bg-[#070707] text-gray-600">
        <span className="animate-pulse">読み込み中...</span>
      </div>
    );
  }

  if (isError) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-2 bg-[#070707] text-red-500 text-sm">
        <span>データの読み込みに失敗しました</span>
        <span className="text-xs text-red-700 max-w-lg text-center break-all">{String(error)}</span>
      </div>
    );
  }

  if (works.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3 bg-[#070707] text-gray-600">
        <p className="text-sm">作品がありません</p>
        <p className="text-xs text-gray-700">ソース管理からフォルダを追加してください</p>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col overflow-hidden bg-black">
      {isSelectMode && <BulkActionBar />}
      {viewMode === "grid" ? (
        <div className="flex-1 overflow-y-auto p-4 bg-[#070707]">
          <div className="grid gap-3" style={{ gridTemplateColumns: `repeat(auto-fill, minmax(${GRID_MIN_WIDTH[density]}, 1fr))` }}>
            {works.map((work) => (
              <WorkCard
                key={work.id}
                work={work}
                selected={selectedWorkId === work.id}
                onSelect={() => setSelectedWorkId(selectedWorkId === work.id ? null : work.id)}
              />
            ))}
          </div>
        </div>
      ) : (
        <>
          <LibraryBrowser works={works} />
          <VirtualList
            works={works}
            selectedWorkId={selectedWorkId}
            setSelectedWorkId={setSelectedWorkId}
            density={density}
            isSelectMode={isSelectMode}
            selectedWorkIds={selectedWorkIds}
            selectAllWorks={selectAllWorks}
            clearSelection={clearSelection}
          />
        </>
      )}
    </div>
  );
}
