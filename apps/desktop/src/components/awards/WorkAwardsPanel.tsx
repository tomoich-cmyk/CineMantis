import { useState } from "react";
import { AwardTierBadge, ResultBadge } from "@/components/awards/AwardBadge";
import { WorkAwardForm } from "@/components/awards/WorkAwardForm";
import { useDeleteWorkAwardResult, useUpdateWorkAwardResult, useWorkAwards } from "@/hooks/useAwards";

export function WorkAwardsPanel({
  workId,
  defaultYear,
}: {
  workId: number;
  defaultYear?: number | null;
}) {
  const { data: awards = [], isLoading } = useWorkAwards(workId);
  const deleteAward = useDeleteWorkAwardResult(workId);
  const updateAward = useUpdateWorkAwardResult(workId);
  const [formOpen, setFormOpen] = useState(false);

  function remove(id: number) {
    if (!window.confirm("この映画賞情報を削除しますか？")) return;
    deleteAward.mutate(id, {
      onError: (error) => window.alert(String(error)),
    });
  }

  return (
    <section className="border-t border-subtle">
      <div className="flex items-center justify-between px-3 py-2">
        <span className="text-xs font-medium text-gray-400">映画賞・映画祭</span>
        <button
          type="button"
          onClick={() => setFormOpen((value) => !value)}
          className="text-xs text-mantis-400 hover:text-mantis-300"
        >
          {formOpen ? "閉じる" : "追加"}
        </button>
      </div>

      {isLoading && <p className="px-3 pb-2 text-xs text-gray-600">読み込み中...</p>}

      {!isLoading && awards.length === 0 && !formOpen && (
        <p className="px-3 pb-2 text-xs text-gray-600">登録なし</p>
      )}

      {awards.length > 0 && (
        <div className="space-y-1 px-3 pb-2">
          {awards.map((award) => (
            <div key={award.id} className="group rounded bg-surface/60 px-2 py-1.5">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    <AwardTierBadge tier={award.prestigeTier} />
                    <span className="truncate text-xs font-medium text-gray-200">
                      {award.awardBodyDisplayNameJa}
                    </span>
                    {award.awardYear && <span className="text-[11px] text-gray-500">{award.awardYear}</span>}
                  </div>
                  <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[11px] text-gray-500">
                    <ResultBadge result={award.resultType} />
                    <span>{award.categoryDisplayNameJa}</span>
                    {award.personName && <span>・{award.personName}</span>}
                    {award.sourceUrl && (
                      <a
                        href={award.sourceUrl}
                        target="_blank"
                        rel="noreferrer"
                        className="text-blue-400 hover:text-blue-300"
                      >
                        出典
                      </a>
                    )}
                  </div>
                  {award.note && <p className="mt-1 text-[11px] text-gray-600">{award.note}</p>}
                </div>
                <div className="flex flex-col items-end gap-1">
                  <label className="flex items-center gap-1 text-[10px] text-gray-600">
                    <input
                      type="checkbox"
                      checked={award.isLocked}
                      onChange={(event) =>
                        updateAward.mutate(
                          { id: award.id, isLocked: event.target.checked },
                          { onError: (error) => window.alert(String(error)) },
                        )
                      }
                      className="accent-mantis-500"
                    />
                    固定
                  </label>
                  {!award.isLocked && (
                    <button
                      type="button"
                      onClick={() => remove(award.id)}
                      className="text-[11px] text-gray-700 opacity-0 transition-opacity hover:text-red-400 group-hover:opacity-100"
                    >
                      削除
                    </button>
                  )}
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {formOpen && (
        <WorkAwardForm workId={workId} defaultYear={defaultYear} onDone={() => setFormOpen(false)} />
      )}
    </section>
  );
}
