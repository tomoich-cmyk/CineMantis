import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useLibraryStore, DEFAULT_COLUMN_WIDTHS, DEFAULT_VISIBLE_COLUMNS } from "@/store/libraryStore";
import type { Density } from "@/store/libraryStore";
import { useUpdateStats, useUpdateWorkLibraryFields, useWorkList } from "@/hooks/useWorks";
import { WorkCard } from "./WorkCard";
import { WorkRow } from "./WorkRow";
import { BulkActionBar } from "./BulkActionBar";
import { useSetWorkMatchStatus } from "@/hooks/useBulk";
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

export const COLUMN_DEFS: { key: string; label: string; sort?: SortField }[] = [
  { key: "title", label: "タイトル", sort: "title" },
  { key: "releaseYear", label: "年", sort: "release_year" },
  { key: "countryType", label: "洋邦", sort: "country_type" },
  { key: "genreText", label: "ジャンル" },
  { key: "myRating", label: "マイ評価", sort: "my_rating" },
  { key: "externalRating", label: "TMDb", sort: "external_rating" },
  { key: "runtime", label: "時間" },
  { key: "matchLocked", label: "固定" },
  { key: "playCount", label: "再生回数", sort: "play_count" },
  { key: "dateAdded", label: "登録日時", sort: "date_added" },
  { key: "lastWatchedAt", label: "再生日時", sort: "last_watched_at" },
  { key: "fileSize", label: "サイズ" },
  { key: "storagePath", label: "保存場所" },
];

const COUNTRY_LABEL: Record<string, string> = {
  foreign: "洋画",
  domestic: "邦画",
  unknown: "不明",
};

const RATING_PRESETS = [10, 9.5, 9, 8.5, 8, 7.5, 7, 6.5, 6, 5, 4, 3, 2, 1, 0];

function normalizeRating(value: string | number) {
  const rating = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(rating)) return null;
  const clamped = Math.min(10, Math.max(0, rating));
  return Math.round(clamped * 10) / 10;
}

function formatRatingDraft(value: number | null | undefined) {
  return typeof value === "number" && Number.isFinite(value) ? normalizeRating(value)?.toFixed(1) ?? "" : "";
}

function decadeOf(work: WorkSummary) {
  const year = work.releaseYear ?? work.year;
  if (!year) return "不明";
  return String(year);
}

function splitGenres(value: string | null) {
  if (!value) return ["未設定"];
  return value
    .replace(/^\[|\]$/g, "")
    .split(/[,\u3001/]/)
    .map((v) => v.replace(/^"|"$/g, "").trim())
    .filter(Boolean)
    .slice(0, 8);
}

function makeCounts(
  works: WorkSummary[],
  getValues: (work: WorkSummary) => string[],
  sorter?: (a: [string, number], b: [string, number]) => number,
) {
  const counts = new Map<string, number>();
  for (const work of works) {
    for (const value of getValues(work)) {
      counts.set(value, (counts.get(value) ?? 0) + 1);
    }
  }
  return Array.from(counts.entries()).sort(sorter ?? countSort);
}

function countSort(a: [string, number], b: [string, number]) {
  const aUnknown = a[0] === "不明" || a[0] === "未設定";
  const bUnknown = b[0] === "不明" || b[0] === "未設定";
  if (aUnknown !== bUnknown) return aUnknown ? 1 : -1;
  return b[1] - a[1] || a[0].localeCompare(b[0], "ja");
}

function yearSort(a: [string, number], b: [string, number]) {
  const aYear = Number(a[0]);
  const bYear = Number(b[0]);
  const aUnknown = Number.isNaN(aYear);
  const bUnknown = Number.isNaN(bYear);
  if (aUnknown !== bUnknown) return aUnknown ? 1 : -1;
  if (!aUnknown && aYear !== bYear) return bYear - aYear;
  return countSort(a, b);
}

function ratingBucket(work: WorkSummary) {
  const rating = work.myRating ?? work.userRating;
  if (!rating) return "未評価";
  return `★ ${rating.toFixed(1)}`;
}

function ratingSort(a: [string, number], b: [string, number]) {
  if (a[0] === "未評価") return 1;
  if (b[0] === "未評価") return -1;
  return Number.parseFloat(b[0].replace("★", "").trim()) - Number.parseFloat(a[0].replace("★", "").trim());
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
  const total = items.reduce((sum, [, count]) => sum + count, 0);
  return (
    <section className="min-w-0 flex flex-col overflow-hidden border-r border-[#151515] bg-[#070707]">
      <div className="h-7 flex items-center px-3 text-xs text-gray-500 border-b border-[#151515]">{title}</div>
      <div className="flex-1 overflow-auto">
        <button
          onClick={() => onPick(null)}
          className={`w-full grid grid-cols-[2px_1fr_auto] gap-2 px-2 py-1 text-left text-xs ${active === null ? "bg-[#101510] text-mantis-300" : "text-gray-300 hover:bg-[#141414]"}`}
        >
          <span className={active === null ? "bg-mantis-500" : ""} />
          <span className="truncate">すべて</span>
          <span>{total.toLocaleString("ja-JP")}</span>
        </button>
        {items.map(([label, count]) => (
          <button
            key={label}
            onClick={() => onPick(label)}
            className={`w-full grid grid-cols-[2px_1fr_auto] gap-2 px-2 py-1 text-left text-xs ${active === label ? "bg-[#101510] text-mantis-300" : "text-gray-400 hover:bg-[#141414] hover:text-gray-100"}`}
          >
            <span className={active === label ? "bg-mantis-500" : ""} />
            <span className="truncate">{label}</span>
            <span>{count.toLocaleString("ja-JP")}</span>
          </button>
        ))}
      </div>
    </section>
  );
}

function LibraryBrowser({ works }: { works: WorkSummary[] }) {
  const {
    filters,
    setFilter,
    browserPaneHeight,
    setBrowserPaneHeight,
  } = useLibraryStore();
  const resizeRef = useRef<{ startY: number; startHeight: number } | null>(null);

  const countryCounts = useMemo(
    () => makeCounts(works, (work) => [COUNTRY_LABEL[work.countryType] ?? work.countryType ?? "不明"]),
    [works],
  );
  const decadeCounts = useMemo(() => makeCounts(works, (work) => [decadeOf(work)], yearSort), [works]);
  const ratingCounts = useMemo(() => makeCounts(works, (work) => [ratingBucket(work)], ratingSort), [works]);
  const genreCounts = useMemo(() => makeCounts(works, (work) => splitGenres(work.genreText)), [works]);

  useEffect(() => {
    function onMove(event: MouseEvent) {
      const current = resizeRef.current;
      if (!current) return;
      setBrowserPaneHeight(current.startHeight + event.clientY - current.startY);
    }
    function onUp() {
      resizeRef.current = null;
      document.body.classList.remove("cm-row-resizing");
    }
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [setBrowserPaneHeight]);

  function startResize(event: React.MouseEvent) {
    event.preventDefault();
    resizeRef.current = { startY: event.clientY, startHeight: browserPaneHeight };
    document.body.classList.add("cm-row-resizing");
  }

  const countryReverse: Record<string, string> = { 洋画: "foreign", 邦画: "domestic", 不明: "unknown" };

  return (
    <div className="relative flex-shrink-0 overflow-hidden border-b border-[#151515] bg-black" style={{ height: browserPaneHeight }}>
      <div className="h-full grid grid-cols-[0.85fr_0.75fr_0.9fr_1.55fr]">
        <BrowserPane
          title="洋邦"
          items={countryCounts}
          active={filters.countryType ? COUNTRY_LABEL[filters.countryType] : null}
          onPick={(value) => setFilter("countryType", value ? countryReverse[value] ?? null : null)}
        />
        <BrowserPane
          title="年"
          items={decadeCounts}
          active={filters.yearFrom !== null && filters.yearTo !== null && filters.yearFrom === filters.yearTo ? String(filters.yearFrom) : null}
          onPick={(value) => {
            if (!value || value === "不明") {
              setFilter("yearFrom", null);
              setFilter("yearTo", null);
              return;
            }
            const year = Number(value);
            setFilter("yearFrom", year);
            setFilter("yearTo", year);
          }}
        />
        <BrowserPane
          title="評価"
          items={ratingCounts}
          active={filters.minUserRating ? `★ ${filters.minUserRating.toFixed(1)}` : null}
          onPick={(value) => setFilter("minUserRating", value && value !== "未評価" ? Number(value.replace("★", "").trim()) : null)}
        />
        <BrowserPane
          title="ジャンル"
          items={genreCounts}
          active={filters.genre}
          onPick={(value) => setFilter("genre", value === "未設定" ? null : value)}
        />
      </div>
      <button
        type="button"
        aria-label="ブラウザ行の高さを変更"
        onMouseDown={startResize}
        className="absolute bottom-0 left-0 right-0 h-px cursor-row-resize bg-[#1a1a1a] hover:bg-mantis-500/60"
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
  const columnDragRef = useRef<{ sourceKey: string; targetKey: string; startX: number; active: boolean } | null>(null);
  const [columnMenuOpen, setColumnMenuOpen] = useState(false);
  const [draggedColumn, setDraggedColumn] = useState<string | null>(null);
  const [columnDropTarget, setColumnDropTarget] = useState<string | null>(null);
  const suppressHeaderClickRef = useRef(false);
  const [lastSelectedIndex, setLastSelectedIndex] = useState<number | null>(null);
  const [editMenu, setEditMenu] = useState<{ x: number; y: number; work: WorkSummary; workIds: number[] } | null>(null);
  const [ratingDraft, setRatingDraft] = useState("");
  const { mutate: updateLibraryFields } = useUpdateWorkLibraryFields();
  const { mutate: updateStats } = useUpdateStats();
  const { mutate: setWorkMatchStatus } = useSetWorkMatchStatus();
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

  const visibleDefs = visibleColumns
    .map((key) => COLUMN_DEFS.find((column) => column.key === key))
    .filter((column): column is (typeof COLUMN_DEFS)[number] => column !== undefined);
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

  useEffect(() => {
    function onMove(event: MouseEvent) {
      const current = columnDragRef.current;
      if (!current) return;
      if (!current.active && Math.abs(event.clientX - current.startX) < 5) return;

      current.active = true;
      event.preventDefault();
      setDraggedColumn(current.sourceKey);

      const headers = Array.from(
        document.querySelectorAll<HTMLElement>("[data-library-column-key]"),
      );
      const target = headers.find((header) => {
        const rect = header.getBoundingClientRect();
        return event.clientX >= rect.left && event.clientX <= rect.right;
      });
      const targetKey = target?.dataset.libraryColumnKey;
      if (targetKey) {
        current.targetKey = targetKey;
        setColumnDropTarget(targetKey);
      }
    }

    function onUp() {
      const current = columnDragRef.current;
      columnDragRef.current = null;
      if (!current?.active) return;

      const from = visibleColumns.indexOf(current.sourceKey);
      const to = visibleColumns.indexOf(current.targetKey);
      if (from !== -1 && to !== -1 && from !== to) {
        const next = [...visibleColumns];
        next.splice(from, 1);
        next.splice(to, 0, current.sourceKey);
        setVisibleColumns(next);
      }

      suppressHeaderClickRef.current = true;
      window.setTimeout(() => {
        suppressHeaderClickRef.current = false;
      }, 0);
      setDraggedColumn(null);
      setColumnDropTarget(null);
    }

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [setVisibleColumns, visibleColumns]);

  useEffect(() => {
    if (!editMenu) return;
    const close = () => setEditMenu(null);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("blur", close);
    };
  }, [editMenu]);

  const items = virtualizer.getVirtualItems();
  const allIds = works.map((w) => w.id);
  const allSelected = allIds.length > 0 && allIds.every((id) => selectedWorkIds.includes(id));

  function toggleSort(sort: SortField | undefined) {
    if (suppressHeaderClickRef.current) {
      suppressHeaderClickRef.current = false;
      return;
    }
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

  function prepareColumnDrag(event: React.MouseEvent, key: string) {
    if (event.button !== 0) return;
    columnDragRef.current = {
      sourceKey: key,
      targetKey: key,
      startX: event.clientX,
      active: false,
    };
  }

  function toggleWorkSelection(id: number, shiftKey: boolean, additive: boolean) {
    const index = works.findIndex((work) => work.id === id);
    if (index === -1) return;

    if (shiftKey && lastSelectedIndex !== null) {
      const start = Math.min(lastSelectedIndex, index);
      const end = Math.max(lastSelectedIndex, index);
      const rangeIds = works.slice(start, end + 1).map((work) => work.id);
      const next = new Set(additive ? selectedWorkIds : []);
      for (const rangeId of rangeIds) next.add(rangeId);
      selectAllWorks(Array.from(next));
    } else if (additive || isSelectMode) {
      const next = selectedWorkIds.includes(id)
        ? selectedWorkIds.filter((selectedId) => selectedId !== id)
        : [...selectedWorkIds, id];
      selectAllWorks(next);
    } else {
      selectAllWorks([id]);
    }
    setSelectedWorkId(id);
    setLastSelectedIndex(index);
  }

  function openEditMenu(event: React.MouseEvent, work: WorkSummary) {
    const workIds = selectedWorkIds.includes(work.id) ? selectedWorkIds : [work.id];
    if (!selectedWorkIds.includes(work.id)) selectAllWorks([work.id]);
    setSelectedWorkId(work.id);
    setRatingDraft(formatRatingDraft(work.myRating ?? work.userRating));
    setEditMenu({
      x: Math.min(event.clientX, window.innerWidth - 300),
      y: Math.min(event.clientY, window.innerHeight - 430),
      work,
      workIds,
    });
  }

  function editTextField(field: "reading" | "genre_text" | "release_year") {
    if (!editMenu) return;
    const current = field === "reading"
      ? editMenu.work.reading ?? ""
      : field === "genre_text"
        ? editMenu.work.genreText ?? ""
        : String(editMenu.work.releaseYear ?? editMenu.work.year ?? "");
    const labels = { reading: "よみ", genre_text: "ジャンル", release_year: "年" };
    const input = window.prompt(`${labels[field]}を入力`, current);
    if (input === null) return;
    const next = input.trim();
    if (field === "release_year") {
      const year = next ? Number(next) : null;
      if (year !== null && (!Number.isInteger(year) || year < 0)) return;
      for (const workId of editMenu.workIds) updateLibraryFields({ work_id: workId, release_year: year });
    } else {
      for (const workId of editMenu.workIds) updateLibraryFields({ work_id: workId, [field]: next || null });
    }
    setEditMenu(null);
  }

  function setCountry(countryType: "foreign" | "domestic" | "unknown") {
    if (!editMenu) return;
    for (const workId of editMenu.workIds) updateLibraryFields({ work_id: workId, country_type: countryType });
    setEditMenu(null);
  }

  function setRating(rating: number | null) {
    if (!editMenu) return;
    const nextRating = rating === null ? null : normalizeRating(rating);
    if (nextRating === null) return;
    for (const workId of editMenu.workIds) updateStats({ work_id: workId, user_rating: nextRating, my_rating: nextRating });
    setEditMenu(null);
  }

  function applyRatingDraft() {
    const nextRating = normalizeRating(ratingDraft);
    if (nextRating === null) return;
    setRating(nextRating);
  }

  function editPlayCount() {
    if (!editMenu) return;
    const input = window.prompt("再生回数を入力", String(editMenu.work.playCount));
    if (input === null) return;
    const playCount = Number(input.trim());
    if (!Number.isInteger(playCount) || playCount < 0) return;
    for (const workId of editMenu.workIds) updateStats({ work_id: workId, play_count: playCount });
    setEditMenu(null);
  }

  return (
    <div className="flex-1 flex flex-col overflow-hidden bg-[#070707]">
      <div className="h-8 flex items-center justify-between px-3 border-b border-[#151515] bg-[#0b0b0b]">
        <div className="text-xs text-gray-500">作品: {works.length.toLocaleString("ja-JP")} 件</div>
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
              <div
                key={column.key}
                data-library-column-key={column.key}
                className={`relative flex items-center border-r border-[#1b1b1b] ${
                  draggedColumn === column.key ? "opacity-40" : ""
                } ${columnDropTarget === column.key && draggedColumn !== column.key ? "before:absolute before:inset-y-0 before:left-0 before:w-px before:bg-mantis-400" : ""}`}
              >
                <button
                  type="button"
                  onMouseDown={(event) => prepareColumnDrag(event, column.key)}
                  onClick={() => toggleSort(column.sort)}
                  className={`w-full h-full px-2 text-left truncate cursor-grab active:cursor-grabbing ${column.sort ? "hover:text-gray-200" : ""}`}
                  title="左右にドラッグして列を移動"
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
                  onSelect={() => {
                    clearSelection();
                    setLastSelectedIndex(vItem.index);
                    setSelectedWorkId(selectedWorkId === work.id ? null : work.id);
                  }}
                  onToggleSelect={toggleWorkSelection}
                  onShiftContextMenu={openEditMenu}
                  onToggleMatchLock={(workId, locked) =>
                    setWorkMatchStatus({ workId, status: locked ? "locked" : "matched" })
                  }
                  gridTemplateColumns={gridTemplateColumns}
                  visibleColumns={visibleDefs.map((column) => column.key)}
                  top={vItem.start}
                  height={vItem.size}
                />
              );
            })}
          </div>
        </div>
      </div>
      {editMenu && (
        <div
          onPointerDown={(event) => event.stopPropagation()}
          className="fixed z-[80] w-72 border border-[#333] bg-[#171717] py-1 text-xs text-gray-200 shadow-2xl"
          style={{ left: editMenu.x, top: editMenu.y }}
        >
          <div className="border-b border-[#2b2b2b] px-3 py-1.5 text-[11px] text-gray-500">
            編集 {editMenu.workIds.length > 1 ? `(${editMenu.workIds.length}件)` : ""}
          </div>
          <button onClick={() => editTextField("reading")} className="w-full px-3 py-1.5 text-left hover:bg-[#303030]">よみを編集…</button>
          <button onClick={() => editTextField("release_year")} className="w-full px-3 py-1.5 text-left hover:bg-[#303030]">年を編集…</button>
          <button onClick={() => editTextField("genre_text")} className="w-full px-3 py-1.5 text-left hover:bg-[#303030]">ジャンルを編集…</button>
          <button onClick={editPlayCount} className="w-full px-3 py-1.5 text-left hover:bg-[#303030]">再生回数を編集…</button>
          <div className="my-1 border-t border-[#2b2b2b]" />
          <div className="px-3 py-1 text-[10px] text-gray-500">洋邦</div>
          <div className="grid grid-cols-3 gap-1 px-2 pb-1">
            <button onClick={() => setCountry("foreign")} className="border border-[#333] py-1 hover:bg-[#303030]">洋画</button>
            <button onClick={() => setCountry("domestic")} className="border border-[#333] py-1 hover:bg-[#303030]">邦画</button>
            <button onClick={() => setCountry("unknown")} className="border border-[#333] py-1 hover:bg-[#303030]">不明</button>
          </div>
          <div className="px-3 py-1 text-[10px] text-gray-500">マイ評価 / 10</div>
          <div className="px-2 pb-2">
            <div className="flex items-center gap-2">
              <span className="text-sm text-yellow-400">★</span>
              <input
                type="number"
                min={0}
                max={10}
                step={0.1}
                value={ratingDraft}
                onChange={(event) => setRatingDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") applyRatingDraft();
                }}
                className="h-8 min-w-0 flex-1 border border-[#2c2c2c] bg-[#101014] px-2 text-right text-sm text-gray-100 outline-none focus:border-mantis-500"
                placeholder="0.0"
              />
              <button
                onClick={applyRatingDraft}
                className="h-8 border border-mantis-700 px-3 text-mantis-200 hover:bg-mantis-900/40"
              >
                適用
              </button>
            </div>
            <input
              type="range"
              min={0}
              max={10}
              step={0.1}
              value={normalizeRating(ratingDraft) ?? 0}
              onChange={(event) => setRatingDraft(Number(event.target.value).toFixed(1))}
              className="mt-2 w-full accent-mantis-500"
            />
          </div>
          <div className="grid grid-cols-5 gap-1 px-2 pb-1">
            {RATING_PRESETS.map((rating) => (
              <button
                key={rating}
                onClick={() => setRating(rating)}
                className="border border-[#333] py-1 font-mono text-[11px] hover:bg-[#303030]"
              >
                {rating.toFixed(1)}
              </button>
            ))}
          </div>
          <button onClick={() => setRating(0)} className="w-full px-3 py-1.5 text-left text-gray-500 hover:bg-[#303030]">0.0 にする</button>
        </div>
      )}
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
