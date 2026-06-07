import { useRef, useEffect, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useLibraryStore, DEFAULT_VISIBLE_COLUMNS } from "@/store/libraryStore";
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
  compact: 36,
  normal: 50,
  relaxed: 64,
};

const COLUMN_DEFS: { key: string; label: string; sort?: SortField; width: string }[] = [
  { key: "title", label: "タイトル", sort: "title", width: "min-w-[220px]" },
  { key: "releaseYear", label: "年", sort: "release_year", width: "w-20" },
  { key: "mediaCategory", label: "種別", sort: "media_category", width: "w-24" },
  { key: "countryType", label: "洋邦", sort: "country_type", width: "w-20" },
  { key: "genreText", label: "ジャンル", width: "min-w-[140px]" },
  { key: "myRating", label: "評価", sort: "my_rating", width: "w-28" },
  { key: "watchedStatus", label: "視聴状態", sort: "watched_status", width: "w-24" },
  { key: "dateAdded", label: "登録日", sort: "date_added", width: "w-28" },
  { key: "lastWatchedAt", label: "最終視聴日", sort: "last_watched_at", width: "w-28" },
  { key: "storagePath", label: "保存場所", width: "min-w-[220px]" },
];

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
  const [columnMenuOpen, setColumnMenuOpen] = useState(false);
  const { sortField, sortOrder, setSortField, setSortOrder, visibleColumns, setVisibleColumns } = useLibraryStore();

  const virtualizer = useVirtualizer({
    count: works.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT[density],
    overscan: 10,
    getItemKey: (i) => works[i].id,
  });

  useEffect(() => {
    if (selectedWorkId === null) return;
    const idx = works.findIndex((w) => w.id === selectedWorkId);
    if (idx !== -1) {
      virtualizer.scrollToIndex(idx, { align: "auto", behavior: "smooth" });
    }
  }, [selectedWorkId]);

  const items = virtualizer.getVirtualItems();
  const paddingTop = items.length > 0 ? items[0].start : 0;
  const paddingBottom = items.length > 0 ? virtualizer.getTotalSize() - items[items.length - 1].end : 0;
  const allIds = works.map((w) => w.id);
  const allSelected = allIds.length > 0 && allIds.every((id) => selectedWorkIds.includes(id));
  const visibleDefs = COLUMN_DEFS.filter((c) => visibleColumns.includes(c.key));
  const colSpan = visibleDefs.length + (isSelectMode ? 1 : 0);

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

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      <div className="flex items-center justify-between px-3 py-2 border-b border-subtle bg-surface">
        <div className="text-xs text-gray-500">{works.length.toLocaleString("ja-JP")} 件</div>
        <div className="relative">
          <button
            onClick={() => setColumnMenuOpen((v) => !v)}
            className="px-2 py-1 text-xs border border-subtle rounded text-gray-400 hover:text-gray-200"
          >
            列
          </button>
          {columnMenuOpen && (
            <div className="absolute right-0 mt-1 w-40 rounded border border-subtle bg-surface-elevated shadow-xl z-20 p-1">
              {COLUMN_DEFS.map((column) => (
                <label key={column.key} className="flex items-center gap-2 px-2 py-1 text-xs text-gray-400 hover:bg-surface-hover rounded">
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

      <div ref={scrollRef} className="flex-1 overflow-auto">
        <table className="w-full text-sm border-collapse table-auto">
          <thead className="sticky top-0 bg-surface-elevated border-b border-subtle z-10">
            <tr>
              {isSelectMode && (
                <th className="pl-3 pr-1 py-2 w-8">
                  <button
                    onClick={() => (allSelected ? clearSelection() : selectAllWorks(allIds))}
                    className="w-4 h-4 rounded border-2 border-gray-500 flex items-center justify-center text-[9px] text-white"
                    style={{ background: allSelected ? "#2563eb" : "transparent", borderColor: allSelected ? "#2563eb" : undefined }}
                    title={allSelected ? "全解除" : "全選択"}
                  >
                    {allSelected ? "✓" : ""}
                  </button>
                </th>
              )}
              {visibleDefs.map((column) => (
                <th
                  key={column.key}
                  className={`text-left px-3 py-2 text-gray-400 font-medium ${column.width}`}
                >
                  <button
                    type="button"
                    onClick={() => toggleSort(column.sort)}
                    className={column.sort ? "hover:text-gray-100" : "cursor-default"}
                  >
                    {column.label}
                    {column.sort && sortField === column.sort && (
                      <span className="ml-1 text-mantis-400">{sortOrder === "asc" ? "↑" : "↓"}</span>
                    )}
                  </button>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {paddingTop > 0 && (
              <tr aria-hidden="true">
                <td colSpan={colSpan} style={{ height: paddingTop, padding: 0, border: 0 }} />
              </tr>
            )}
            {items.map((vItem) => {
              const work = works[vItem.index];
              return (
                <WorkRow
                  key={work.id}
                  work={work}
                  selected={selectedWorkId === work.id}
                  onSelect={() => setSelectedWorkId(selectedWorkId === work.id ? null : work.id)}
                />
              );
            })}
            {paddingBottom > 0 && (
              <tr aria-hidden="true">
                <td colSpan={colSpan} style={{ height: paddingBottom, padding: 0, border: 0 }} />
              </tr>
            )}
          </tbody>
        </table>
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
      <div className="flex-1 flex items-center justify-center text-gray-600">
        <span className="animate-pulse">読み込み中...</span>
      </div>
    );
  }

  if (isError) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-2 text-red-500 text-sm">
        <span>データの読み込みに失敗しました</span>
        <span className="text-xs text-red-700 max-w-lg text-center break-all">{String(error)}</span>
      </div>
    );
  }

  if (works.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3 text-gray-600">
        <p className="text-sm">作品がありません</p>
        <p className="text-xs text-gray-700">ソース管理からフォルダを追加してください</p>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {isSelectMode && <BulkActionBar />}

      {viewMode === "grid" ? (
        <div className="flex-1 overflow-y-auto p-4">
          <div
            className="grid gap-3"
            style={{ gridTemplateColumns: `repeat(auto-fill, minmax(${GRID_MIN_WIDTH[density]}, 1fr))` }}
          >
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
      )}
    </div>
  );
}
