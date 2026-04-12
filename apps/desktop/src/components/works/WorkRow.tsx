import { clsx } from "clsx";
import type { WorkSummary } from "@cinemantis/shared-types";
import { StarRating } from "@/components/common/StarRating";

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
  return (
    <tr
      onClick={onSelect}
      className={clsx(
        "border-b border-subtle cursor-pointer transition-colors",
        selected
          ? "bg-mantis-600/10 text-gray-100"
          : "hover:bg-surface-hover text-gray-300"
      )}
    >
      <td className="px-4 py-2">
        <div className="flex items-center gap-2">
          {work.isFavorite && <span className="text-xs">⭐</span>}
          <span className="font-medium">{work.title}</span>
        </div>
      </td>
      <td className="px-3 py-2 text-gray-500">{work.year ?? "—"}</td>
      <td className="px-3 py-2">
        {work.userRating !== null ? (
          <StarRating value={work.userRating} size="xs" readonly />
        ) : (
          <span className="text-gray-600">—</span>
        )}
      </td>
      <td className="px-3 py-2 text-gray-500">{work.playCount}</td>
      <td className="px-3 py-2">
        <span
          className={clsx(
            "text-xs px-1.5 py-0.5 rounded",
            work.watchStatus === "watched" && "bg-mantis-900/60 text-mantis-400",
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
