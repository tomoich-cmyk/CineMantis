import { clsx } from "clsx";
import type { WorkSummary } from "@cinemantis/shared-types";
import { StarRating } from "@/components/common/StarRating";
import { useLibraryStore } from "@/store/libraryStore";
import type { Density } from "@/store/libraryStore";

// 密度 → 行の縦パディング
const ROW_PY: Record<Density, string> = {
  compact: "py-1",
  normal:  "py-2",
  relaxed: "py-3",
};

interface Props {
  work: WorkSummary;
  selected: boolean;
  onSelect: () => void;
}

const WATCH_STATUS_LABEL: Record<string, string> = {
  watched: "視聴済",
  watching: "視聴中",
  unwatched: "未視聴",
  skipped: "スキップ",
};

export function WorkRow({ work, selected, onSelect }: Props) {
  const { isSelectMode, selectedWorkIds, toggleSelectWork, density } = useLibraryStore();
  const isChecked = selectedWorkIds.includes(work.id);
  const rowPy = ROW_PY[density];

  function handleClick() {
    if (isSelectMode) {
      toggleSelectWork(work.id);
    } else {
      onSelect();
    }
  }

  return (
    <tr
      onClick={handleClick}
      className={clsx(
        "border-b border-subtle cursor-pointer transition-colors",
        isSelectMode && isChecked
          ? "bg-blue-900/20 text-gray-100"
          : selected
          ? "bg-mantis-600/10 text-gray-100"
          : "hover:bg-surface-hover text-gray-300"
      )}
    >
      {/* チェックボックスカラム（選択モード時のみ） */}
      {isSelectMode && (
        <td className={clsx("pl-3 pr-1 w-8", rowPy)}>
          <div className={clsx(
            "w-4 h-4 rounded border-2 flex items-center justify-center transition-colors",
            isChecked ? "bg-blue-600 border-blue-600 text-white" : "border-gray-500"
          )}>
            {isChecked && <span className="text-[9px] leading-none">✓</span>}
          </div>
        </td>
      )}

      <td className={clsx("px-4", rowPy)}>
        <div className="flex items-center gap-2">
          {work.isFavorite && <span className="text-xs">⭐</span>}
          <span className="font-medium">{work.title}</span>
          {work.watchStatus === "watching" && work.resumePositionSec !== null && work.runtimeSec !== null && work.runtimeSec > 0 && (
            <span className="text-[11px] text-blue-400">
              ⏱ {Math.round((work.resumePositionSec / work.runtimeSec) * 100)}%
            </span>
          )}
        </div>
      </td>
      <td className={clsx("px-3 text-gray-500", rowPy)}>{work.year ?? "—"}</td>
      <td className={clsx("px-3", rowPy)}>
        {work.userRating !== null ? (
          <StarRating value={work.userRating} size="xs" readonly />
        ) : (
          <span className="text-gray-600">—</span>
        )}
      </td>
      <td className={clsx("px-3 text-gray-500", rowPy)}>{work.playCount}</td>
      <td className={clsx("px-3", rowPy)}>
        <span
          className={clsx(
            "text-xs px-1.5 py-0.5 rounded",
            work.watchStatus === "watched"  && "bg-mantis-900/60 text-mantis-400",
            work.watchStatus === "watching" && "bg-blue-900/60 text-blue-400",
            work.watchStatus === "unwatched" && "bg-surface-border text-gray-500",
          )}
        >
          {WATCH_STATUS_LABEL[work.watchStatus] ?? work.watchStatus}
        </span>
      </td>
    </tr>
  );
}
