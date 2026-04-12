import { clsx } from "clsx";
import type { WorkSummary } from "@cinemantis/shared-types";
import { StarRating } from "@/components/common/StarRating";

interface Props {
  work: WorkSummary;
  selected: boolean;
  onSelect: () => void;
}

export function WorkCard({ work, selected, onSelect }: Props) {
  return (
    <div
      onClick={onSelect}
      className={clsx(
        "group flex flex-col cursor-pointer rounded overflow-hidden border transition-all",
        selected
          ? "border-mantis-500 ring-1 ring-mantis-500/40"
          : "border-surface-border hover:border-gray-600"
      )}
    >
      {/* Poster */}
      <div className="aspect-[2/3] bg-surface-hover flex items-center justify-center relative">
        {work.posterPath ? (
          <img
            src={work.posterPath}
            alt={work.title}
            className="w-full h-full object-cover"
          />
        ) : (
          <span className="text-3xl opacity-30">🎬</span>
        )}
        {work.isFavorite && (
          <span className="absolute top-1 right-1 text-xs">⭐</span>
        )}
        {work.watchStatus === "unwatched" && (
          <div className="absolute bottom-0 left-0 right-0 h-0.5 bg-mantis-600/60" />
        )}
      </div>

      {/* Info */}
      <div className="px-2 py-1.5 bg-surface-elevated flex flex-col gap-0.5">
        <p className="text-xs font-medium text-gray-200 line-clamp-2 leading-tight">
          {work.title}
        </p>
        <div className="flex items-center justify-between">
          <span className="text-xs text-gray-500">{work.year ?? "—"}</span>
          {work.userRating !== null && (
            <StarRating value={work.userRating} size="xs" readonly />
          )}
        </div>
      </div>
    </div>
  );
}
