import { AwardTierBadge } from "@/components/awards/AwardBadge";
import { useAwardBodies } from "@/hooks/useAwards";
import { useLibraryStore } from "@/store/libraryStore";

const TYPE_LABELS: Record<string, string> = {
  industry_award: "業界賞",
  film_festival: "映画祭",
  national_academy: "国内賞",
  regional_award: "地域賞",
  critics_award: "批評家賞",
  other: "その他",
};

function alertClass(status: string): string {
  switch (status) {
    case "up_to_date":
      return "border-mantis-700 bg-mantis-900/30 text-mantis-300";
    case "data_stale":
    case "result_season":
      return "border-yellow-700 bg-yellow-900/20 text-yellow-300";
    case "nomination_season":
    case "nomination_soon":
      return "border-blue-700 bg-blue-900/20 text-blue-300";
    default:
      return "border-subtle bg-surface text-gray-500";
  }
}

export function AwardsScreen() {
  const { data: bodies = [], isLoading, error } = useAwardBodies();
  const { setSelectedAwardBodyId } = useLibraryStore();

  return (
    <main className="flex-1 overflow-hidden bg-surface">
      <header className="flex items-center justify-between border-b border-subtle px-5 py-3">
        <div>
          <h1 className="text-base font-semibold text-gray-100">映画賞・映画祭</h1>
          <p className="mt-0.5 text-xs text-gray-600">所有作品に紐づく受賞・ノミネート情報を管理します</p>
        </div>
        <span className="text-xs text-gray-600">{bodies.length} 件</span>
      </header>

      {isLoading && <div className="p-5 text-sm text-gray-500">読み込み中...</div>}
      {error && <div className="p-5 text-sm text-red-400">読み込みに失敗しました</div>}

      {!isLoading && !error && (
        <div className="h-full overflow-auto">
          <table className="w-full table-fixed border-collapse text-sm">
            <thead className="sticky top-0 z-10 bg-surface-elevated text-left text-xs font-medium text-gray-500">
              <tr>
                <th className="w-16 border-b border-subtle px-3 py-2">ランク</th>
                <th className="border-b border-subtle px-3 py-2">名称</th>
                <th className="border-b border-subtle px-3 py-2">英名</th>
                <th className="w-28 border-b border-subtle px-3 py-2">種別</th>
                <th className="w-20 border-b border-subtle px-3 py-2">国</th>
                <th className="w-48 border-b border-subtle px-3 py-2">更新</th>
                <th className="w-24 border-b border-subtle px-3 py-2 text-right">カテゴリ</th>
                <th className="w-24 border-b border-subtle px-3 py-2 text-right">登録作品</th>
                <th className="w-24 border-b border-subtle px-3 py-2 text-right">受賞</th>
              </tr>
            </thead>
            <tbody>
              {bodies.map((body) => (
                <tr
                  key={body.id}
                  onClick={() => setSelectedAwardBodyId(body.id)}
                  className="cursor-pointer border-b border-subtle/70 hover:bg-surface-hover"
                >
                  <td className="px-3 py-2">
                    <AwardTierBadge tier={body.prestigeTier} />
                  </td>
                  <td className="truncate px-3 py-2 font-medium text-gray-200">{body.displayNameJa}</td>
                  <td className="truncate px-3 py-2 text-gray-500">{body.name}</td>
                  <td className="px-3 py-2 text-gray-500">{TYPE_LABELS[body.bodyType] ?? body.bodyType}</td>
                  <td className="px-3 py-2 text-gray-500">{body.country ?? "-"}</td>
                  <td className="px-3 py-2">
                    <span className={`inline-flex max-w-full rounded border px-2 py-0.5 text-[11px] ${alertClass(body.alertStatus)}`}>
                      <span className="truncate">{body.alertLabel}</span>
                    </span>
                  </td>
                  <td className="px-3 py-2 text-right text-mantis-300">{body.categoryCount}</td>
                  <td className="px-3 py-2 text-right text-gray-400">{body.registeredWorkCount}</td>
                  <td className="px-3 py-2 text-right text-yellow-400">{body.winnerCount}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </main>
  );
}
