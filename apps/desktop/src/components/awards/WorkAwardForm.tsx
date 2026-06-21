import { useMemo, useState } from "react";
import type { AwardResultType } from "@cinemantis/shared-types";
import { useAwardBodies, useAwardCategories, useAddWorkAwardResult } from "@/hooks/useAwards";
import { useWorkPersons } from "@/hooks/usePersons";

const RESULT_OPTIONS: { value: AwardResultType; label: string }[] = [
  { value: "winner", label: "受賞" },
  { value: "nominee", label: "ノミネート" },
  { value: "selection", label: "選出" },
  { value: "shortlisted", label: "候補" },
  { value: "special_mention", label: "特別表彰" },
  { value: "unknown", label: "不明" },
];

export function WorkAwardForm({
  workId,
  defaultYear,
  onDone,
}: {
  workId: number;
  defaultYear?: number | null;
  onDone: () => void;
}) {
  const { data: bodies = [] } = useAwardBodies();
  const [awardBodyId, setAwardBodyId] = useState<number | null>(null);
  const { data: categories = [] } = useAwardCategories(awardBodyId);
  const { data: persons = [] } = useWorkPersons(workId);
  const addAward = useAddWorkAwardResult(workId);

  const [awardYear, setAwardYear] = useState(defaultYear?.toString() ?? "");
  const [awardCategoryId, setAwardCategoryId] = useState<number | null>(null);
  const [resultType, setResultType] = useState<AwardResultType>("winner");
  const [personId, setPersonId] = useState<number | null>(null);
  const [sourceUrl, setSourceUrl] = useState("");
  const [note, setNote] = useState("");
  const [isLocked, setIsLocked] = useState(false);

  const personOptions = useMemo(() => {
    const seen = new Set<number>();
    return persons.filter((person) => {
      if (seen.has(person.person_id)) return false;
      seen.add(person.person_id);
      return true;
    });
  }, [persons]);

  function submit() {
    if (!awardBodyId || !awardCategoryId) return;
    addAward.mutate(
      {
        workId,
        awardBodyId,
        awardCategoryId,
        awardYear: awardYear ? Number(awardYear) : null,
        resultType,
        personId,
        sourceUrl: sourceUrl || null,
        note: note || null,
        isLocked,
      },
      {
        onSuccess: onDone,
        onError: (error) => window.alert(String(error)),
      },
    );
  }

  return (
    <div className="space-y-2 border-t border-subtle px-3 py-2">
      <div className="grid grid-cols-2 gap-1.5">
        <label className="col-span-2 flex flex-col gap-1 text-xs text-gray-500">
          映画賞・映画祭
          <select
            value={awardBodyId ?? ""}
            onChange={(event) => {
              const next = event.target.value ? Number(event.target.value) : null;
              setAwardBodyId(next);
              setAwardCategoryId(null);
            }}
            className="bg-surface border border-subtle rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-mantis-600"
          >
            <option value="">選択</option>
            {bodies.map((body) => (
              <option key={body.id} value={body.id}>
                {body.prestigeTier} {body.displayNameJa}
              </option>
            ))}
          </select>
        </label>

        <label className="flex flex-col gap-1 text-xs text-gray-500">
          年
          <input
            type="number"
            value={awardYear}
            onChange={(event) => setAwardYear(event.target.value)}
            className="bg-surface border border-subtle rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-mantis-600"
          />
        </label>

        <label className="flex flex-col gap-1 text-xs text-gray-500">
          結果
          <select
            value={resultType}
            onChange={(event) => setResultType(event.target.value as AwardResultType)}
            className="bg-surface border border-subtle rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-mantis-600"
          >
            {RESULT_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </label>

        <label className="col-span-2 flex flex-col gap-1 text-xs text-gray-500">
          カテゴリ
          <select
            value={awardCategoryId ?? ""}
            onChange={(event) => setAwardCategoryId(event.target.value ? Number(event.target.value) : null)}
            disabled={!awardBodyId}
            className="bg-surface border border-subtle rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-mantis-600 disabled:opacity-40"
          >
            <option value="">選択</option>
            {categories.map((category) => (
              <option key={category.id} value={category.id}>
                {category.displayNameJa}
              </option>
            ))}
          </select>
        </label>

        <label className="col-span-2 flex flex-col gap-1 text-xs text-gray-500">
          人物
          <select
            value={personId ?? ""}
            onChange={(event) => setPersonId(event.target.value ? Number(event.target.value) : null)}
            className="bg-surface border border-subtle rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-mantis-600"
          >
            <option value="">作品として登録</option>
            {personOptions.map((person) => (
              <option key={person.person_id} value={person.person_id}>
                {person.name}
              </option>
            ))}
          </select>
        </label>

        <label className="col-span-2 flex flex-col gap-1 text-xs text-gray-500">
          出典URL
          <input
            value={sourceUrl}
            onChange={(event) => setSourceUrl(event.target.value)}
            className="bg-surface border border-subtle rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-mantis-600"
          />
        </label>

        <label className="col-span-2 flex flex-col gap-1 text-xs text-gray-500">
          メモ
          <input
            value={note}
            onChange={(event) => setNote(event.target.value)}
            className="bg-surface border border-subtle rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-mantis-600"
          />
        </label>
      </div>

      <label className="flex items-center gap-2 text-xs text-gray-500">
        <input
          type="checkbox"
          checked={isLocked}
          onChange={(event) => setIsLocked(event.target.checked)}
          className="accent-mantis-500"
        />
        この候補で固定する
      </label>

      <div className="flex justify-end gap-2">
        <button onClick={onDone} className="px-2 py-1 text-xs text-gray-500 hover:text-gray-200">
          キャンセル
        </button>
        <button
          onClick={submit}
          disabled={!awardBodyId || !awardCategoryId || addAward.isPending}
          className="rounded border border-mantis-700 bg-mantis-800/40 px-3 py-1 text-xs text-mantis-300 hover:bg-mantis-800/70 disabled:opacity-40"
        >
          {addAward.isPending ? "保存中..." : "追加"}
        </button>
      </div>
    </div>
  );
}
