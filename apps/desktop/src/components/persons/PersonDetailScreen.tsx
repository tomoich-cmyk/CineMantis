import { useState } from "react";
import { clsx } from "clsx";
import { usePersonDetail, usePersonWorks } from "@/hooks/usePersons";
import { useLibraryStore } from "@/store/libraryStore";
import { WorkCard } from "@/components/works/WorkCard";
import { StarRating } from "@/components/common/StarRating";
import type { WorkSummary } from "@cinemantis/shared-types";

const ROLE_TABS = [
  { value: null,       label: "すべて" },
  { value: "director", label: "監督" },
  { value: "writer",   label: "脚本" },
  { value: "cast",     label: "出演" },
] as const;

const WATCH_STATUS_LABEL: Record<string, string> = {
  watched: "視聴済",
  watching: "視聴中",
  unwatched: "未視聴",
  skipped: "スキップ",
};

export function PersonDetailScreen({ personId }: { personId: number }) {
  const {
    setSelectedPersonId,
    setSelectedWorkId,
    selectedWorkId,
    viewMode,
    setViewMode,
  } = useLibraryStore();

  const [roleFilter, setRoleFilter] = useState<string | null>(null);

  const { data: person } = usePersonDetail(personId);
  const { data: works = [], isLoading } = usePersonWorks(personId, roleFilter);

  return (
    <div className="flex-1 flex flex-col min-h-0 overflow-hidden">
      {/* Header */}
      <div className="flex items-center gap-3 px-4 py-3 border-b border-subtle bg-surface-elevated flex-shrink-0">
        {/* Back */}
        <button
          onClick={() => setSelectedPersonId(null)}
          className="text-gray-500 hover:text-gray-200 text-sm transition-colors"
        >
          ← 一覧
        </button>

        <div className="w-px h-4 bg-surface-border" />

        {/* Avatar + name */}
        <div className="w-8 h-8 flex-shrink-0 rounded-full bg-surface flex items-center justify-center text-sm font-medium text-gray-400">
          {person?.name.slice(0, 1) ?? "?"}
        </div>

        <div className="flex-1 min-w-0">
          <h1 className="text-sm font-semibold text-gray-200 truncate">
            {person?.name ?? "…"}
          </h1>
          <p className="text-xs text-gray-600">{works.length} 作品</p>
        </div>

        {/* Role filter */}
        <div className="flex gap-1">
          {ROLE_TABS.map((tab) => (
            <button
              key={String(tab.value)}
              onClick={() => setRoleFilter(tab.value)}
              className={clsx(
                "px-2 py-0.5 text-xs rounded transition-colors",
                roleFilter === tab.value
                  ? "bg-mantis-700/40 text-mantis-300"
                  : "text-gray-500 hover:text-gray-300"
              )}
            >
              {tab.label}
            </button>
          ))}
        </div>

        {/* View mode */}
        <div className="flex border border-subtle rounded overflow-hidden">
          <button
            onClick={() => setViewMode("grid")}
            className={clsx(
              "px-2 py-1 text-sm transition-colors",
              viewMode === "grid"
                ? "bg-mantis-700/40 text-mantis-300"
                : "text-gray-500 hover:text-gray-300"
            )}
          >
            ⊞
          </button>
          <button
            onClick={() => setViewMode("list")}
            className={clsx(
              "px-2 py-1 text-sm border-l border-subtle transition-colors",
              viewMode === "list"
                ? "bg-mantis-700/40 text-mantis-300"
                : "text-gray-500 hover:text-gray-300"
            )}
          >
            ≡
          </button>
        </div>
      </div>

      {/* Works */}
      <div className="flex-1 overflow-y-auto">
        {isLoading ? (
          <div className="flex items-center justify-center h-40 text-gray-600 animate-pulse text-sm">
            読み込み中…
          </div>
        ) : works.length === 0 ? (
          <div className="flex items-center justify-center h-40 text-gray-600 text-sm">
            作品がありません
          </div>
        ) : viewMode === "grid" ? (
          <div className="p-4">
            <div className="grid grid-cols-[repeat(auto-fill,minmax(130px,1fr))] gap-3">
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
          <table className="w-full text-sm border-collapse">
            <thead className="sticky top-0 bg-surface-elevated border-b border-subtle z-10">
              <tr>
                <th className="text-left px-4 py-2 text-gray-400 font-medium">タイトル</th>
                <th className="text-left px-3 py-2 text-gray-400 font-medium w-16">年</th>
                <th className="text-left px-3 py-2 text-gray-400 font-medium w-20">評価</th>
                <th className="text-left px-3 py-2 text-gray-400 font-medium w-16">視聴</th>
                <th className="text-left px-3 py-2 text-gray-400 font-medium w-24">状態</th>
              </tr>
            </thead>
            <tbody>
              {works.map((work) => (
                <PersonWorkRow
                  key={work.id}
                  work={work}
                  selected={selectedWorkId === work.id}
                  onSelect={() =>
                    setSelectedWorkId(selectedWorkId === work.id ? null : work.id)
                  }
                />
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

function PersonWorkRow({
  work,
  selected,
  onSelect,
}: {
  work: WorkSummary;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <tr
      onClick={onSelect}
      className={clsx(
        "cursor-pointer border-b border-subtle transition-colors",
        selected ? "bg-mantis-600/10 text-gray-100" : "hover:bg-surface-hover text-gray-300"
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
