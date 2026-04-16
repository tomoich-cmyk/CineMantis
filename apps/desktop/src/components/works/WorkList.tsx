import { useRef, useEffect } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useLibraryStore } from "@/store/libraryStore";
import type { Density } from "@/store/libraryStore";
import { useWorkList } from "@/hooks/useWorks";
import { WorkCard } from "./WorkCard";
import { WorkRow } from "./WorkRow";
import { BulkActionBar } from "./BulkActionBar";
import type { WorkSummary } from "@cinemantis/shared-types";

// ─── 密度設定 ─────────────────────────────────────────────────────────────────

// 密度 → グリッドの最小カラム幅
const GRID_MIN_WIDTH: Record<Density, string> = {
  compact: "110px",
  normal:  "140px",
  relaxed: "180px",
};

// 密度 → リスト行の推定高さ（固定値: virtualizer に渡す）
const ROW_HEIGHT: Record<Density, number> = {
  compact: 36,
  normal:  50,
  relaxed: 64,
};

// ─── VirtualList ─────────────────────────────────────────────────────────────

/**
 * パディング行方式の仮想スクロールテーブル。
 * tbody の先頭と末尾に高さだけを持つ空 tr を置いてスクロール高を確保し、
 * 表示範囲内の行だけ実際の WorkRow をレンダリングする。
 * WorkRow の変更は不要で、density による行高変動にも自動追従する。
 */
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

  const virtualizer = useVirtualizer({
    count: works.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT[density],
    overscan: 10,
    getItemKey: (i) => works[i].id,
  });

  // 選択中の作品が変わったとき、その行が見えていなければスクロールで追従する
  useEffect(() => {
    if (selectedWorkId === null) return;
    const idx = works.findIndex((w) => w.id === selectedWorkId);
    if (idx !== -1) {
      virtualizer.scrollToIndex(idx, { align: "auto", behavior: "smooth" });
    }
  }, [selectedWorkId]); // eslint-disable-line react-hooks/exhaustive-deps

  const items = virtualizer.getVirtualItems();

  // 仮想範囲の前後に挿入するパディング高
  const paddingTop    = items.length > 0 ? items[0].start : 0;
  const paddingBottom = items.length > 0
    ? virtualizer.getTotalSize() - items[items.length - 1].end
    : 0;

  const allIds = works.map((w) => w.id);
  const allSelected = allIds.length > 0 && allIds.every((id) => selectedWorkIds.includes(id));
  // 選択列ありの場合は 6 カラム、なしは 5 カラム
  const colSpan = isSelectMode ? 6 : 5;

  return (
    <div ref={scrollRef} className="flex-1 overflow-y-auto">
      <table className="w-full text-sm border-collapse">
        {/* スティッキーヘッダー */}
        <thead className="sticky top-0 bg-surface-elevated border-b border-subtle z-10">
          <tr>
            {isSelectMode && (
              <th className="pl-3 pr-1 py-2 w-8">
                <button
                  onClick={() => allSelected ? clearSelection() : selectAllWorks(allIds)}
                  className="w-4 h-4 rounded border-2 border-gray-500 flex items-center justify-center text-[9px] text-white transition-colors hover:border-blue-400"
                  style={{
                    background:   allSelected ? "#2563eb" : "transparent",
                    borderColor:  allSelected ? "#2563eb" : undefined,
                  }}
                  title={allSelected ? "全解除" : "全選択"}
                >
                  {allSelected ? "✓" : ""}
                </button>
              </th>
            )}
            <th className="text-left px-4 py-2 text-gray-400 font-medium">タイトル</th>
            <th className="text-left px-3 py-2 text-gray-400 font-medium w-16">年</th>
            <th className="text-left px-3 py-2 text-gray-400 font-medium w-20">評価</th>
            <th className="text-left px-3 py-2 text-gray-400 font-medium w-16">視聴</th>
            <th className="text-left px-3 py-2 text-gray-400 font-medium w-24">状態</th>
          </tr>
        </thead>

        <tbody>
          {/* 上部パディング行 */}
          {paddingTop > 0 && (
            <tr aria-hidden="true">
              <td colSpan={colSpan} style={{ height: paddingTop, padding: 0, border: 0 }} />
            </tr>
          )}

          {/* 表示範囲内の行だけレンダリング */}
          {items.map((vItem) => {
            const work = works[vItem.index];
            return (
              <WorkRow
                key={work.id}
                work={work}
                selected={selectedWorkId === work.id}
                onSelect={() =>
                  setSelectedWorkId(selectedWorkId === work.id ? null : work.id)
                }
              />
            );
          })}

          {/* 下部パディング行 */}
          {paddingBottom > 0 && (
            <tr aria-hidden="true">
              <td colSpan={colSpan} style={{ height: paddingBottom, padding: 0, border: 0 }} />
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

// ─── WorkList ─────────────────────────────────────────────────────────────────

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
        <span className="animate-pulse">読み込み中…</span>
      </div>
    );
  }

  if (isError) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-2 text-red-500 text-sm">
        <span>データの読み込みに失敗しました</span>
        <span className="text-xs text-red-700 max-w-lg text-center break-all">
          {String(error)}
        </span>
      </div>
    );
  }

  if (works.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3 text-gray-600">
        <span className="text-5xl opacity-30">🎬</span>
        <p className="text-sm">作品がありません</p>
        <p className="text-xs text-gray-700">
          左メニューの「ソース管理」からフォルダを追加してください
        </p>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* 一括操作バー（選択モード時） */}
      {isSelectMode && <BulkActionBar />}

      {viewMode === "grid" ? (
        /* ── グリッド表示（仮想化なし） ── */
        <div className="flex-1 overflow-y-auto p-4">
          <div
            className="grid gap-3"
            style={{
              gridTemplateColumns: `repeat(auto-fill, minmax(${GRID_MIN_WIDTH[density]}, 1fr))`,
            }}
          >
            {works.map((work) => (
              <WorkCard
                key={work.id}
                work={work}
                selected={selectedWorkId === work.id}
                onSelect={() =>
                  setSelectedWorkId(selectedWorkId === work.id ? null : work.id)
                }
              />
            ))}
          </div>
        </div>
      ) : (
        /* ── リスト表示（仮想スクロール） ── */
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
