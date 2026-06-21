import { AwardTierBadge, ResultBadge } from "@/components/awards/AwardBadge";
import { useAwardBodyDetail, useAwardWinningWorks } from "@/hooks/useAwards";
import { useLibraryStore } from "@/store/libraryStore";

export function AwardDetailScreen({ awardBodyId }: { awardBodyId: number }) {
  const { data: body, isLoading: bodyLoading } = useAwardBodyDetail(awardBodyId);
  const { data: rows = [], isLoading: rowsLoading } = useAwardWinningWorks({ awardBodyId });
  const { setSelectedAwardBodyId, setActiveSection, setSelectedWorkId } = useLibraryStore();

  function openWork(workId: number) {
    setSelectedWorkId(workId);
    setActiveSection("all-movies");
  }

  return (
    <main className="flex-1 overflow-hidden bg-surface">
      <header className="flex items-center justify-between border-b border-subtle px-5 py-3">
        <div className="min-w-0">
          <button
            onClick={() => setSelectedAwardBodyId(null)}
            className="mb-1 text-xs text-gray-500 hover:text-gray-300"
          >
            ← 一覧へ
          </button>
          {bodyLoading || !body ? (
            <h1 className="text-base font-semibold text-gray-100">読み込み中...</h1>
          ) : (
            <div className="flex items-center gap-2">
              <AwardTierBadge tier={body.prestigeTier} />
              <h1 className="truncate text-base font-semibold text-gray-100">{body.displayNameJa}</h1>
              <span className="truncate text-xs text-gray-600">{body.name}</span>
            </div>
          )}
        </div>
        {body && (
          <div className="flex gap-4 text-xs text-gray-500">
            <span>作品 {body.registeredWorkCount}</span>
            <span className="text-yellow-400">受賞 {body.winnerCount}</span>
          </div>
        )}
      </header>

      {rowsLoading && <div className="p-5 text-sm text-gray-500">読み込み中...</div>}
      {!rowsLoading && rows.length === 0 && (
        <div className="p-5 text-sm text-gray-600">この賞に紐づく作品はまだありません。</div>
      )}

      {!rowsLoading && rows.length > 0 && (
        <div className="h-full overflow-auto">
          <table className="w-full table-fixed border-collapse text-sm">
            <thead className="sticky top-0 z-10 bg-surface-elevated text-left text-xs font-medium text-gray-500">
              <tr>
                <th className="w-24 border-b border-subtle px-3 py-2">開催年</th>
                <th className="border-b border-subtle px-3 py-2">作品</th>
                <th className="w-24 border-b border-subtle px-3 py-2">公開年</th>
                <th className="w-56 border-b border-subtle px-3 py-2">カテゴリ</th>
                <th className="w-28 border-b border-subtle px-3 py-2">結果</th>
                <th className="w-44 border-b border-subtle px-3 py-2">人物</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr
                  key={row.resultId}
                  onClick={() => openWork(row.workId)}
                  className="cursor-pointer border-b border-subtle/70 hover:bg-surface-hover"
                >
                  <td className="px-3 py-2 text-gray-400">{row.awardYear ?? "-"}</td>
                  <td className="truncate px-3 py-2 font-medium text-gray-200">{row.title}</td>
                  <td className="px-3 py-2 text-gray-500">{row.year ?? "-"}</td>
                  <td className="truncate px-3 py-2 text-gray-400">{row.categoryDisplayNameJa}</td>
                  <td className="px-3 py-2"><ResultBadge result={row.resultType} /></td>
                  <td className="truncate px-3 py-2 text-gray-500">{row.personName ?? "-"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </main>
  );
}
