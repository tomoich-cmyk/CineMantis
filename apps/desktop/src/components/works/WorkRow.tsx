import { clsx } from "clsx";
import type { WorkSummary } from "@cinemantis/shared-types";
import { StarRating } from "@/components/common/StarRating";
import { useLibraryStore } from "@/store/libraryStore";

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
  gridTemplateColumns: string;
  top: number;
  height: number;
}

function formatDate(value: string | null) {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.slice(0, 16);
  return date.toLocaleDateString("ja-JP");
}

function cellClass(extra = "") {
  return `min-w-0 flex items-center px-2 border-r border-[#151515] truncate ${extra}`;
}

function renderCell(column: string, work: WorkSummary) {
  switch (column) {
    case "title":
      return (
        <div className="min-w-0">
          <div className="truncate text-gray-200">{work.title || "(無題)"}</div>
          {work.watchedStatus === "watching" && work.resumePositionSec !== null && work.runtimeSec !== null && work.runtimeSec > 0 && (
            <div className="text-[10px] text-blue-400">{Math.round((work.resumePositionSec / work.runtimeSec) * 100)}%</div>
          )}
        </div>
      );
    case "releaseYear":
      return work.releaseYear ?? work.year ?? "";
    case "mediaCategory":
      return CATEGORY_LABEL[work.mediaCategory] ?? work.mediaCategory ?? "";
    case "countryType":
      return COUNTRY_LABEL[work.countryType] ?? work.countryType ?? "";
    case "genreText":
      return <span className="truncate">{work.genreText || ""}</span>;
    case "myRating":
      return work.myRating !== null || work.userRating !== null ? (
        <StarRating value={work.myRating ?? work.userRating} size="xs" readonly />
      ) : (
        <span className="text-gray-700">未評価</span>
      );
    case "watchedStatus":
      return (
        <span
          className={clsx(
            "text-[11px] px-1.5 py-0.5",
            work.watchedStatus === "watched" && "text-mantis-300",
            work.watchedStatus === "watching" && "text-blue-300",
            work.watchedStatus === "abandoned" && "text-orange-300",
            work.watchedStatus === "unwatched" && "text-gray-600",
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
      return <span className="truncate text-gray-500">{work.storagePath || ""}</span>;
    default:
      return null;
  }
}

export function WorkRow({ work, selected, onSelect, gridTemplateColumns, top, height }: Props) {
  const { isSelectMode, selectedWorkIds, toggleSelectWork, visibleColumns } = useLibraryStore();
  const isChecked = selectedWorkIds.includes(work.id);

  function handleClick() {
    if (isSelectMode) {
      toggleSelectWork(work.id);
    } else {
      onSelect();
    }
  }

  return (
    <div
      onClick={handleClick}
      className={clsx(
        "absolute left-0 right-0 grid text-xs border-b border-[#101010] cursor-default",
        isSelectMode && isChecked
          ? "bg-blue-900/25 text-gray-100"
          : selected
            ? "bg-mantis-500/18 text-gray-100"
            : "text-gray-400 hover:bg-[#141414]",
      )}
      style={{ top, height, gridTemplateColumns }}
    >
      {isSelectMode && (
        <div className="flex items-center justify-center border-r border-[#151515]">
          <div className={clsx("w-3.5 h-3.5 border border-gray-600", isChecked && "bg-blue-500 border-blue-500 text-white flex items-center justify-center")}>
            {isChecked && <span className="text-[9px]">✓</span>}
          </div>
        </div>
      )}

      {visibleColumns.map((column) => (
        <div key={column} className={cellClass(column === "title" ? "font-medium" : "")}>
          {renderCell(column, work)}
        </div>
      ))}
    </div>
  );
}
