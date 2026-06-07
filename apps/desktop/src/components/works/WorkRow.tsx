import { useState } from "react";
import { clsx } from "clsx";
import type { WorkSummary } from "@cinemantis/shared-types";
import { StarRating } from "@/components/common/StarRating";
import { useLibraryStore } from "@/store/libraryStore";
import { useUpdateStats, useUpdateWorkLibraryFields } from "@/hooks/useWorks";

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

const CATEGORY_OPTIONS = [
  ["movie", "映画"],
  ["drama", "ドラマ"],
  ["ova", "OVA"],
  ["other", "その他"],
] as const;

const COUNTRY_OPTIONS = [
  ["foreign", "洋画"],
  ["domestic", "邦画"],
  ["unknown", "不明"],
] as const;

const EDITABLE_COLUMNS = new Set(["title", "releaseYear", "mediaCategory", "countryType", "genreText", "myRating"]);

interface Props {
  work: WorkSummary;
  selected: boolean;
  onSelect: () => void;
  gridTemplateColumns: string;
  visibleColumns: string[];
  top: number;
  height: number;
}

function formatDateTime(value: string | null) {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.slice(0, 16);
  return date.toLocaleString("ja-JP", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatBytes(value: number | null) {
  if (value === null || value <= 0) return "";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(unit >= 3 ? 1 : 0)} ${units[unit]}`;
}

function formatGenres(value: string | null) {
  if (!value) return "";
  try {
    const parsed = JSON.parse(value);
    if (Array.isArray(parsed)) return parsed.filter(Boolean).join(", ");
  } catch {
    // Plain text genre values are common for manually edited records.
  }
  return value
    .replace(/^\[|\]$/g, "")
    .split(/[,\u3001/]/)
    .map((v) => v.replace(/^"|"$/g, "").trim())
    .filter(Boolean)
    .join(", ");
}

function cellClass(extra = "") {
  return `min-w-0 flex items-center px-2 border-r border-[#151515] truncate ${extra}`;
}

function editorClass(extra = "") {
  return `w-full h-[calc(100%-4px)] bg-[#10131a] border border-mantis-700/70 px-1.5 text-xs text-gray-100 outline-none ${extra}`;
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
      return <span className="truncate">{formatGenres(work.genreText)}</span>;
    case "myRating":
      return work.myRating !== null || work.userRating !== null ? (
        <StarRating value={work.myRating ?? work.userRating} size="xs" readonly />
      ) : (
        <span className="text-gray-700">未評価</span>
      );
    case "playCount":
      return <span className="font-mono text-gray-500">{work.playCount.toLocaleString("ja-JP")}</span>;
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
      return formatDateTime(work.dateAdded);
    case "lastWatchedAt":
      return formatDateTime(work.lastWatchedAt);
    case "fileSize":
      return <span className="font-mono text-gray-500">{formatBytes(work.fileSize)}</span>;
    case "storagePath":
      return <span className="truncate text-gray-500">{work.storagePath || ""}</span>;
    default:
      return null;
  }
}

export function WorkRow({ work, selected, onSelect, gridTemplateColumns, visibleColumns, top, height }: Props) {
  const { isSelectMode, selectedWorkIds, toggleSelectWork } = useLibraryStore();
  const { mutate: updateLibraryFields } = useUpdateWorkLibraryFields();
  const { mutate: updateStats } = useUpdateStats();
  const [editingColumn, setEditingColumn] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const isChecked = selectedWorkIds.includes(work.id);

  function handleClick() {
    if (isSelectMode) {
      toggleSelectWork(work.id);
    } else {
      onSelect();
    }
  }

  function editableValue(column: string) {
    switch (column) {
      case "title":
        return work.title;
      case "releaseYear":
        return String(work.releaseYear ?? work.year ?? "");
      case "mediaCategory":
        return work.mediaCategory ?? "other";
      case "countryType":
        return work.countryType ?? "unknown";
      case "genreText":
        return formatGenres(work.genreText);
      case "myRating":
        return String(work.myRating ?? work.userRating ?? "");
      default:
        return "";
    }
  }

  function startEdit(column: string, event: React.MouseEvent) {
    if (!EDITABLE_COLUMNS.has(column)) return;
    event.preventDefault();
    event.stopPropagation();
    setEditingColumn(column);
    setDraft(editableValue(column));
  }

  function cancelEdit() {
    setEditingColumn(null);
    setDraft("");
  }

  function commitEdit(column: string, value = draft) {
    if (editingColumn !== column) return;
    const trimmed = value.trim();
    setEditingColumn(null);
    setDraft("");

    if (column === "title") {
      if (!trimmed || trimmed === work.title) return;
      updateLibraryFields({ work_id: work.id, title: trimmed });
      return;
    }

    if (column === "releaseYear") {
      const next = trimmed ? Number(trimmed) : null;
      if (next !== null && (!Number.isInteger(next) || next < 0)) return;
      if (next === (work.releaseYear ?? work.year ?? null)) return;
      updateLibraryFields({ work_id: work.id, release_year: next });
      return;
    }

    if (column === "mediaCategory") {
      if (value === work.mediaCategory) return;
      updateLibraryFields({ work_id: work.id, media_category: value });
      return;
    }

    if (column === "countryType") {
      if (value === work.countryType) return;
      updateLibraryFields({ work_id: work.id, country_type: value });
      return;
    }

    if (column === "genreText") {
      const next = trimmed || null;
      if (next === (work.genreText ?? null)) return;
      updateLibraryFields({ work_id: work.id, genre_text: next });
      return;
    }

    if (column === "myRating") {
      const next = trimmed ? Number(trimmed) : null;
      if (next !== null && (!Number.isFinite(next) || next < 1 || next > 5)) return;
      if (next === (work.myRating ?? work.userRating ?? null)) return;
      updateStats({ work_id: work.id, user_rating: next, my_rating: next });
    }
  }

  function handleEditorKey(event: React.KeyboardEvent<HTMLInputElement | HTMLSelectElement>) {
    if (!editingColumn) return;
    if (event.key === "Enter") {
      event.preventDefault();
      commitEdit(editingColumn);
    }
    if (event.key === "Escape") {
      event.preventDefault();
      cancelEdit();
    }
  }

  function renderEditor(column: string) {
    if (column === "mediaCategory") {
      return (
        <select
          autoFocus
          value={draft}
          onClick={(event) => event.stopPropagation()}
          onKeyDown={handleEditorKey}
          onBlur={() => commitEdit(column)}
          onChange={(event) => {
            setDraft(event.target.value);
            commitEdit(column, event.target.value);
          }}
          className={editorClass()}
        >
          {CATEGORY_OPTIONS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
        </select>
      );
    }

    if (column === "countryType") {
      return (
        <select
          autoFocus
          value={draft}
          onClick={(event) => event.stopPropagation()}
          onKeyDown={handleEditorKey}
          onBlur={() => commitEdit(column)}
          onChange={(event) => {
            setDraft(event.target.value);
            commitEdit(column, event.target.value);
          }}
          className={editorClass()}
        >
          {COUNTRY_OPTIONS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
        </select>
      );
    }

    return (
      <input
        autoFocus
        type={column === "releaseYear" || column === "myRating" ? "number" : "text"}
        min={column === "myRating" ? 1 : undefined}
        max={column === "myRating" ? 5 : undefined}
        value={draft}
        onClick={(event) => event.stopPropagation()}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={handleEditorKey}
        onBlur={() => commitEdit(column)}
        className={editorClass(column === "title" || column === "genreText" ? "" : "text-center")}
      />
    );
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
        <div
          key={column}
          onDoubleClick={(event) => startEdit(column, event)}
          title={EDITABLE_COLUMNS.has(column) ? "ダブルクリックで編集" : undefined}
          className={cellClass(column === "title" ? "font-medium" : "")}
        >
          {editingColumn === column ? renderEditor(column) : renderCell(column, work)}
        </div>
      ))}
    </div>
  );
}
