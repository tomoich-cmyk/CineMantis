import { useEffect, useMemo, useState } from "react";
import { clsx } from "clsx";
import { useSourceList } from "@/hooks/useSources";
import { useNasSortPlan, useExecuteNasSort } from "@/hooks/useNasSort";
import type { NasSortPlanRow, NasSortStatus, NasSortResult } from "@/api/nasSort";
import type { Source } from "@cinemantis/shared-types";

const STATUS_META: Record<
  NasSortStatus,
  { label: string; tone: string; hint: string }
> = {
  ready:        { label: "移動可",     tone: "text-mantis-400",  hint: "" },
  no_reading:   { label: "よみ未設定", tone: "text-amber-400",   hint: "未整理画面でよみを入力すると移動できます" },
  no_country:   { label: "洋邦未設定", tone: "text-amber-400",   hint: "詳細パネルで洋画／邦画を選ぶと移動できます" },
  dest_exists:  { label: "重複あり",   tone: "text-red-400",     hint: "移動先に同名ファイルがあります" },
  file_missing: { label: "実体なし",   tone: "text-red-400",     hint: "元ファイルが見つかりません" },
  same_path:    { label: "配置済み",   tone: "text-gray-500",    hint: "すでに正しい場所にあります" },
};

function formatSize(bytes: number | null): string {
  if (bytes === null) return "—";
  const gb = bytes / 1024 ** 3;
  if (gb >= 1) return `${gb.toFixed(1)} GB`;
  return `${(bytes / 1024 ** 2).toFixed(0)} MB`;
}

function ResultBanner({ result }: { result: NasSortResult }) {
  return (
    <div className="border border-subtle bg-surface-elevated px-4 py-3 text-sm">
      <div className="flex gap-4">
        <span className="text-mantis-400">移動 {result.moved}</span>
        {result.failed > 0 && <span className="text-red-400">失敗 {result.failed}</span>}
        {result.skipped > 0 && <span className="text-gray-500">対象外 {result.skipped}</span>}
      </div>
      {result.errors.length > 0 && (
        <ul className="mt-2 space-y-1">
          {result.errors.map((e) => (
            <li key={e.work_id} className="text-xs text-red-300">
              {e.title}: {e.message}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function NasSortScreen() {
  const { data: sources } = useSourceList() as { data: Source[] | undefined };
  const [sourceId, setSourceId] = useState<number | null>(null);
  const [checked, setChecked] = useState<Set<number>>(new Set());
  const [result, setResult] = useState<NasSortResult | null>(null);

  const { data: rows, isLoading } = useNasSortPlan(sourceId);
  const execute = useExecuteNasSort(sourceId);

  // 取り込み元は通常ローカル。最初のローカルソースを初期選択する。
  useEffect(() => {
    if (sourceId === null && sources && sources.length > 0) {
      const local = sources.find((s) => s.sourceType === "local") ?? sources[0];
      setSourceId(local.id);
    }
  }, [sources, sourceId]);

  const readyRows = useMemo(
    () => (rows ?? []).filter((r) => r.status === "ready"),
    [rows],
  );

  // ソース切替や再取得で ready でなくなった行の選択は落とす
  useEffect(() => {
    setChecked((prev) => {
      const readyIds = new Set(readyRows.map((r) => r.work_id));
      const next = new Set([...prev].filter((id) => readyIds.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [readyRows]);

  const allReadyChecked = readyRows.length > 0 && checked.size === readyRows.length;

  function toggleAll() {
    setChecked(allReadyChecked ? new Set() : new Set(readyRows.map((r) => r.work_id)));
  }

  function toggle(workId: number) {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(workId)) next.delete(workId);
      else next.add(workId);
      return next;
    });
  }

  async function run() {
    if (checked.size === 0) return;
    const res = await execute.mutateAsync([...checked]);
    setResult(res);
    setChecked(new Set());
  }

  const counts = useMemo(() => {
    const c: Partial<Record<NasSortStatus, number>> = {};
    for (const r of rows ?? []) c[r.status] = (c[r.status] ?? 0) + 1;
    return c;
  }, [rows]);

  return (
    <div className="flex-1 overflow-y-auto p-6 space-y-4">
      <div>
        <h1 className="text-lg font-semibold text-gray-100">NAS 振り分け</h1>
        <p className="mt-1 text-sm text-gray-500">
          取り込みフォルダの作品を、よみに従って NAS の五十音フォルダへ移動します。
          移動先はバックエンドで再計算されるため、一覧の表示と実際の移動先は必ず一致します。
        </p>
      </div>

      <div className="flex items-center gap-3">
        <label className="text-sm text-gray-400">取り込み元</label>
        <select
          value={sourceId ?? ""}
          onChange={(e) => {
            setSourceId(Number(e.target.value));
            setResult(null);
          }}
          className="h-8 min-w-80 bg-[#101014] border border-[#24242a] px-2 text-sm text-gray-200 outline-none focus:border-mantis-500"
        >
          {(sources ?? []).map((s) => (
            <option key={s.id} value={s.id}>
              {s.name}（{s.rootPath}）
            </option>
          ))}
        </select>
      </div>

      {result && <ResultBanner result={result} />}

      <div className="flex items-center gap-4 text-xs text-gray-500">
        <span>対象 {rows?.length ?? 0} 件</span>
        <span className="text-mantis-400">移動可 {counts.ready ?? 0}</span>
        {(counts.no_reading ?? 0) > 0 && (
          <span className="text-amber-400">よみ未設定 {counts.no_reading}</span>
        )}
        {(counts.no_country ?? 0) > 0 && (
          <span className="text-amber-400">洋邦未設定 {counts.no_country}</span>
        )}
        {(counts.dest_exists ?? 0) > 0 && (
          <span className="text-red-400">重複 {counts.dest_exists}</span>
        )}
        {(counts.file_missing ?? 0) > 0 && (
          <span className="text-red-400">実体なし {counts.file_missing}</span>
        )}
      </div>

      <div className="flex items-center gap-3">
        <button
          onClick={toggleAll}
          disabled={readyRows.length === 0}
          className="h-8 px-3 border border-[#24242a] text-sm text-gray-400 hover:text-gray-100 hover:bg-[#121212] disabled:opacity-40 disabled:hover:text-gray-400 disabled:hover:bg-transparent"
        >
          {allReadyChecked ? "選択解除" : "移動可をすべて選択"}
        </button>
        <button
          onClick={run}
          disabled={checked.size === 0 || execute.isPending}
          className="h-8 px-4 bg-mantis-600 text-black text-sm font-medium hover:bg-mantis-500 disabled:opacity-40 disabled:hover:bg-mantis-600"
        >
          {execute.isPending ? "移動中…" : `${checked.size} 件を NAS へ移動`}
        </button>
        {execute.isError && (
          <span className="text-xs text-red-400">
            {(execute.error as Error).message}
          </span>
        )}
      </div>

      {isLoading && <p className="text-sm text-gray-500">読み込み中…</p>}

      {!isLoading && (rows?.length ?? 0) === 0 && (
        <p className="text-sm text-gray-500">
          このソースに移動対象のファイルはありません。
        </p>
      )}

      {(rows?.length ?? 0) > 0 && (
        <table className="w-full text-sm">
          <thead>
            <tr className="text-left text-xs text-gray-500 border-b border-subtle">
              <th className="w-8 py-2"></th>
              <th className="py-2">タイトル</th>
              <th className="py-2 w-20">年</th>
              <th className="py-2 w-40">よみ</th>
              <th className="py-2">移動先</th>
              <th className="py-2 w-24">サイズ</th>
              <th className="py-2 w-28">状態</th>
            </tr>
          </thead>
          <tbody>
            {rows!.map((row: NasSortPlanRow) => {
              const meta = STATUS_META[row.status];
              const selectable = row.status === "ready";
              return (
                <tr
                  key={row.work_id}
                  className={clsx(
                    "border-b border-[#141418]",
                    !selectable && "opacity-60",
                  )}
                >
                  <td className="py-1.5">
                    <input
                      type="checkbox"
                      checked={checked.has(row.work_id)}
                      onChange={() => toggle(row.work_id)}
                      disabled={!selectable}
                      aria-label={`${row.title} を選択`}
                    />
                  </td>
                  <td className="py-1.5 text-gray-200">{row.title}</td>
                  <td className="py-1.5 text-gray-500">{row.year ?? "—"}</td>
                  <td className="py-1.5 text-gray-400">{row.reading ?? "—"}</td>
                  <td className="py-1.5 font-mono text-xs text-gray-500">
                    {row.dest_display ?? row.note ?? "—"}
                  </td>
                  <td className="py-1.5 text-gray-500">{formatSize(row.file_size)}</td>
                  <td className={clsx("py-1.5", meta.tone)} title={row.note ?? meta.hint}>
                    {meta.label}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}
