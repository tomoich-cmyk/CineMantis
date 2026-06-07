import { clsx } from "clsx";
import type { WorkSummary } from "@cinemantis/shared-types";
import { StarRating } from "@/components/common/StarRating";
import { useLibraryStore } from "@/store/libraryStore";
import type { Density } from "@/store/libraryStore";

const ROW_PY: Record<Density, string> = {
  compact: "py-1",
  normal: "py-2",
  relaxed: "py-3",
};

const STATUS_LABEL: Record<string, string> = {
  unwatched: "未視聴",
  watching: "視聴中",
  watched: "視聴済",
  abandoned: "中止",
  skipped: "中止",
};

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

interface Props {
  work: WorkSummary;
  selected: boolean;
  onSelect: () => void;
}

function formatDate(value: string | null) {
  if (!value) return "-";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.slice(0, 10);
  return date.toLocaleDateString("ja-JP");
}

function renderCell(column: string, work: WorkSummary) {
  switch (column) {
    case "title":
      return (
        <div className="min-w-0">
          <div className="font-medium text-gray-100 truncate">{work.title || "(無題)"}</div>
          {work.watchedStatus === "watching" && work.resumePositionSec !== null && work.runtimeSec !== null && work.runtimeSec > 0 && (
            <div className="text-[11px] text-blue-400">
              {Math.round((work.resumePositionSec / work.runtimeSec) * 100)}%
            </div>
          )}
        </div>
      );
    case "releaseYear":
      return work.releaseYear ?? work.year ?? "-";
    case "mediaCategory":
      return CATEGORY_LABEL[work.mediaCategory] ?? work.mediaCategory ?? "-";
    case "countryType":
      return COUNTRY_LABEL[work.countryType] ?? work.countryType ?? "-";
    case "genreText":
      return <span className="line-clamp-1">{work.genreText || "-"}</span>;
    case "myRating":
      return work.myRating !== null || work.userRating !== null ? (
        <StarRating value={work.myRating ?? work.userRating} size="xs" readonly />
      ) : (
        <span className="text-gray-600">-</span>
      );
    case "watchedStatus":
      return (
        <span
          className={clsx(
            "text-xs px-1.5 py-0.5 rounded",
            work.watchedStatus === "watched" && "bg-mantis-900/60 text-mantis-400",
            work.watchedStatus === "watching" && "bg-blue-900/60 text-blue-400",
            work.watchedStatus === "abandoned" && "bg-orange-900/50 text-orange-300",
            work.watchedStatus === "unwatched" && "bg-surface-border text-gray-500",
          )}
        >
          {STATUS_LABEL[work.watchedStatus] ?? work.watchedStatus}
        </span>
      );
    case "dateAdded":
      return formatDate(work.dateAdded);
    case "lastWatchedAt":
      return formatDate(work.lastWatchedAt);
    case "storagePath":
      return <span className="block max-w-[320px] truncate text-gray-500">{work.storagePath || "-"}</span>;
    default:
      return null;
  }
}

export function WorkRow({ work, selected, onSelect }: Props) {
  const { isSelectMode, selectedWorkIds, toggleSelectWork, density, visibleColumns } = useLibraryStore();
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
            : "hover:bg-surface-hover text-gray-300",
      )}
    >
      {isSelectMode && (
        <td className={clsx("pl-3 pr-1 w-8", rowPy)}>
          <div
            className={clsx(
              "w-4 h-4 rounded border-2 flex items-center justify-center transition-colors",
              isChecked ? "bg-blue-600 border-blue-600 text-white" : "border-gray-500",
            )}
          >
            {isChecked && <span className="text-[9px] leading-none">✓</span>}
          </div>
        </td>
      )}

      {visibleColumns.map((column) => (
        <td key={column} className={clsx("px-3 align-middle", rowPy)}>
          {renderCell(column, work)}
        </td>
      ))}
    </tr>
  );
}
