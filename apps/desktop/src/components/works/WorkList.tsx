import { useLibraryStore } from "@/store/libraryStore";
import { useWorkList } from "@/hooks/useWorks";
import { WorkCard } from "./WorkCard";
import { WorkRow } from "./WorkRow";

export function WorkList() {
  const { viewMode, selectedWorkId, setSelectedWorkId } = useLibraryStore();
  const { data: works = [], isLoading, isError } = useWorkList();

  if (isLoading) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-600">
        <span className="animate-pulse">読み込み中…</span>
      </div>
    );
  }

  if (isError) {
    return (
      <div className="flex-1 flex items-center justify-center text-red-500 text-sm">
        データの読み込みに失敗しました
      </div>
    );
  }

  if (works.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3 text-gray-600">
        <span className="text-5xl opacity-30">🎬</span>
        <p className="text-sm">作品がありません</p>
        <p className="text-xs text-gray-700">左メニューの「ソース管理」からフォルダを追加してください</p>
      </div>
    );
  }

  if (viewMode === "grid") {
    return (
      <div className="flex-1 overflow-y-auto p-4">
        <div className="grid grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-3">
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
    );
  }

  return (
    <div className="flex-1 overflow-y-auto">
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
            <WorkRow
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
    </div>
  );
}
