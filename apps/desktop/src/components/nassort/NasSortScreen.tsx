import { useEffect, useMemo, useState } from "react";
import { clsx } from "clsx";
import { open } from "@tauri-apps/plugin-dialog";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNasSortPlan, useExecuteNasSort, nasSortKeys } from "@/hooks/useNasSort";
import { useAddSource, useScanSource, useSourceList } from "@/hooks/useSources";
import { useUpdateWorkLibraryFields, workKeys } from "@/hooks/useWorks";
import { getSetting, setSetting } from "@/api/settings";
import type { NasSortPlanRow, NasSortStatus, NasSortResult } from "@/api/nasSort";
import { CandidateDialog } from "@/components/tmdb/CandidateDialog";

const MATCH_BADGE: Record<string, { label: string; tone: string }> = {
  matched:   { label: "照合済",       tone: "text-mantis-500" },
  locked:    { label: "固定",         tone: "text-yellow-500" },
  pending:   { label: "レビュー待ち", tone: "text-orange-400" },
  unmatched: { label: "未照合",       tone: "text-gray-500" },
};

/** 最後に選んだ取り込み元フォルダ（次回の初期値） */
const LAST_FOLDER_KEY = "nas_sort_last_folder";

const STATUS_META: Record<
  NasSortStatus,
  { label: string; tone: string; hint: string }
> = {
  ready:        { label: "移動可",     tone: "text-mantis-400",  hint: "" },
  no_reading:   { label: "よみ未設定", tone: "text-amber-400",   hint: "よみ欄に入力すると移動できます" },
  no_country:   { label: "洋邦未設定", tone: "text-amber-400",   hint: "移動先欄で洋画／邦画を選ぶと移動できます" },
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

/** 実体があり、まだ配置されていない行は選択できる（洋邦・よみの一括設定用） */
function isSelectable(status: NasSortStatus): boolean {
  return status !== "file_missing" && status !== "same_path";
}

/** よみのインライン入力。Enter かフォーカスを外したときに保存する */
function ReadingInput({
  value,
  disabled,
  onCommit,
}: {
  value: string | null;
  disabled: boolean;
  onCommit: (reading: string) => void;
}) {
  const [draft, setDraft] = useState(value ?? "");
  useEffect(() => setDraft(value ?? ""), [value]);

  function commit() {
    const next = draft.trim();
    if (next && next !== (value ?? "")) onCommit(next);
  }

  return (
    <input
      type="text"
      value={draft}
      disabled={disabled}
      placeholder="よみを入力"
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
      }}
      className={clsx(
        "h-7 w-full bg-transparent border px-1 text-sm outline-none focus:border-mantis-500",
        value ? "border-transparent text-gray-400" : "border-amber-700/60 text-amber-200 placeholder-amber-700",
      )}
    />
  );
}

function ResultBanner({ result }: { result: NasSortResult }) {
  return (
    <div className="border border-subtle bg-surface-elevated px-4 py-3 text-sm">
      <div className="flex gap-4">
        <span className="text-mantis-400">移動 {result.moved}</span>
        {result.failed > 0 && <span className="text-red-400">失敗 {result.failed}</span>}
        {result.skipped > 0 && <span className="text-gray-500">対象外 {result.skipped}</span>}
      </div>
      {result.registered_sources.length > 0 && (
        <p className="mt-2 text-xs text-gray-400">
          移動先をライブラリに登録しました: {result.registered_sources.join("、")}
        </p>
      )}
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
  const [folder, setFolder] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<number>>(new Set());
  const [result, setResult] = useState<NasSortResult | null>(null);
  // 個別照合の対象
  const [matching, setMatching] = useState<NasSortPlanRow | null>(null);

  const { data: plan, isLoading, error: planError } = useNasSortPlan(folder);
  const rows = plan?.rows;
  const execute = useExecuteNasSort(folder);

  // 前回選んだフォルダを初期値にする（自動では選ばない）
  const { data: lastFolder } = useQuery({
    queryKey: ["settings", LAST_FOLDER_KEY],
    queryFn: () => getSetting(LAST_FOLDER_KEY),
  });
  useEffect(() => {
    if (folder === null && lastFolder) setFolder(lastFolder);
  }, [lastFolder, folder]);

  // ── ソース登録とスキャン ──
  const qc = useQueryClient();
  const { data: sources } = useSourceList();
  const addSource = useAddSource();
  const scanSource = useScanSource();
  const [scanError, setScanError] = useState<string | null>(null);
  const scanning = addSource.isPending || scanSource.isPending;
  const containingSource =
    plan?.source_root != null
      ? (sources ?? []).find(
          (s) => s.rootPath.replace(/[\\/]+$/, "").toLowerCase() ===
            plan.source_root!.replace(/[\\/]+$/, "").toLowerCase(),
        ) ?? null
      : null;

  async function scan(sourceId: number) {
    await scanSource.mutateAsync(sourceId);
    await qc.invalidateQueries({ queryKey: nasSortKeys.plan(folder) });
  }

  /** 選んだフォルダをソースとして登録し、そのままスキャンする */
  async function registerAndScan() {
    if (!folder) return;
    setScanError(null);
    try {
      const name = folder.split(/[\\/]/).filter(Boolean).pop() ?? folder;
      const sourceId = await addSource.mutateAsync({
        name,
        root_path: folder,
        source_type: folder.startsWith("\\\\") ? "nas" : "local",
        media_kind: "unknown",
      });
      await scan(sourceId);
    } catch (e) {
      setScanError(String(e));
    }
  }

  async function rescan() {
    if (!containingSource) return;
    setScanError(null);
    try {
      await scan(containingSource.id);
    } catch (e) {
      setScanError(String(e));
    }
  }

  async function chooseFolder() {
    const picked = await open({
      directory: true,
      multiple: false,
      defaultPath: folder ?? undefined,
      title: "取り込み元フォルダを選択",
    });
    if (typeof picked !== "string" || !picked) return;
    setFolder(picked);
    setResult(null);
    setChecked(new Set());
    void setSetting(LAST_FOLDER_KEY, picked);
  }

  const readyRows = useMemo(
    () => (rows ?? []).filter((r) => r.status === "ready"),
    [rows],
  );
  // 洋邦・よみの一括設定のため、実体がある行は ready 以外も選択できる
  const selectableRows = useMemo(
    () => (rows ?? []).filter((r) => isSelectable(r.status)),
    [rows],
  );

  // フォルダ切替や再取得で選択できなくなった行の選択は落とす
  useEffect(() => {
    setChecked((prev) => {
      const ids = new Set(selectableRows.map((r) => r.work_id));
      const next = new Set([...prev].filter((id) => ids.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [selectableRows]);

  const checkedReady = readyRows.filter((r) => checked.has(r.work_id));
  const allReadyChecked = readyRows.length > 0 && checkedReady.length === readyRows.length;
  const allChecked = selectableRows.length > 0 && checked.size === selectableRows.length;

  function toggleAll() {
    setChecked(allReadyChecked ? new Set() : new Set(readyRows.map((r) => r.work_id)));
  }

  function toggleAllSelectable() {
    setChecked(allChecked ? new Set() : new Set(selectableRows.map((r) => r.work_id)));
  }

  // ── 洋邦・よみをこの画面で設定する ──
  const updateFields = useUpdateWorkLibraryFields();
  const [editError, setEditError] = useState<string | null>(null);

  async function updateWorks(workIds: number[], fields: { country_type?: string; reading?: string }) {
    setEditError(null);
    try {
      for (const workId of workIds) {
        await updateFields.mutateAsync({ work_id: workId, ...fields });
      }
    } catch (e) {
      setEditError(String(e));
    } finally {
      await qc.invalidateQueries({ queryKey: nasSortKeys.plan(folder) });
    }
  }

  function setCountryForChecked(countryType: "foreign" | "domestic") {
    if (checked.size === 0) return;
    void updateWorks([...checked], { country_type: countryType });
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
    if (checkedReady.length === 0) return;
    const res = await execute.mutateAsync(checkedReady.map((r) => r.work_id));
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
        <span
          className={clsx(
            "h-8 min-w-80 max-w-2xl truncate border border-[#24242a] bg-[#101014] px-2 font-mono text-xs leading-8",
            folder ? "text-gray-200" : "text-gray-600",
          )}
          title={folder ?? undefined}
        >
          {folder ?? "フォルダが選択されていません"}
        </span>
        <button
          onClick={chooseFolder}
          className="h-8 px-3 border border-[#24242a] text-sm text-gray-300 hover:text-gray-100 hover:bg-[#121212]"
        >
          フォルダを選択…
        </button>
      </div>
      {folder && plan && plan.source_root === null && (
        <div className="flex items-center gap-3">
          <p className="text-xs text-amber-400">
            このフォルダはまだソースに登録されていません。登録してスキャンすると、ここに作品が表示されます。
          </p>
          <button
            onClick={registerAndScan}
            disabled={scanning}
            className="h-8 px-3 bg-mantis-700 text-sm text-white hover:bg-mantis-600 disabled:opacity-40"
          >
            {addSource.isPending ? "登録中…" : scanSource.isPending ? "スキャン中…" : "ソースに登録してスキャン"}
          </button>
        </div>
      )}
      {folder && containingSource && (
        <div className="flex items-center gap-3 text-xs text-gray-500">
          <span className="truncate">
            ソース「{containingSource.name}」（{containingSource.rootPath}）に含まれています
          </span>
          <button
            onClick={rescan}
            disabled={scanning}
            className="h-7 px-3 border border-[#24242a] text-gray-300 hover:text-gray-100 hover:bg-[#121212] disabled:opacity-40"
            title="新しく追加したファイルを取り込みます"
          >
            {scanSource.isPending ? "スキャン中…" : "再スキャン"}
          </button>
        </div>
      )}
      {scanError && <p className="text-xs text-red-400">{scanError}</p>}
      {planError && (
        <p className="text-xs text-red-400">{String(planError)}</p>
      )}

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
          onClick={toggleAllSelectable}
          disabled={selectableRows.length === 0}
          className="h-8 px-3 border border-[#24242a] text-sm text-gray-400 hover:text-gray-100 hover:bg-[#121212] disabled:opacity-40 disabled:hover:text-gray-400 disabled:hover:bg-transparent"
        >
          {allChecked ? "すべて解除" : "すべて選択"}
        </button>
        <button
          onClick={run}
          disabled={checkedReady.length === 0 || execute.isPending}
          className="h-8 px-4 bg-mantis-600 text-black text-sm font-medium hover:bg-mantis-500 disabled:opacity-40 disabled:hover:bg-mantis-600"
        >
          {execute.isPending ? "移動中…" : `${checkedReady.length} 件を NAS へ移動`}
        </button>
        {execute.isError && (
          <span className="text-xs text-red-400">
            {(execute.error as Error).message}
          </span>
        )}
      </div>

      {/* 選択した作品の洋邦を一括設定 */}
      <div className="flex items-center gap-2 text-xs text-gray-500">
        <span>選択した {checked.size} 件の洋邦:</span>
        {(
          [
            ["foreign", "洋画にする"],
            ["domestic", "邦画にする"],
          ] as const
        ).map(([value, label]) => (
          <button
            key={value}
            onClick={() => setCountryForChecked(value)}
            disabled={checked.size === 0 || updateFields.isPending}
            className="h-7 px-3 border border-[#24242a] text-gray-300 hover:text-gray-100 hover:bg-[#121212] disabled:opacity-40"
          >
            {label}
          </button>
        ))}
        {updateFields.isPending && <span>更新中…</span>}
        {editError && <span className="text-red-400">{editError}</span>}
      </div>

      {isLoading && <p className="text-sm text-gray-500">読み込み中…</p>}

      {!folder && (
        <p className="text-sm text-gray-500">
          「フォルダを選択…」で取り込み元のフォルダを選んでください。サブフォルダ内の作品も対象になります。
        </p>
      )}

      {folder && !isLoading && (rows?.length ?? 0) === 0 && (
        <p className="text-sm text-gray-500">
          このフォルダに移動対象のファイルはありません。
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
              const selectable = isSelectable(row.status);
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
                  <td className="py-1.5 text-gray-200">
                    <div className="flex items-center gap-2">
                      <span className="truncate">{row.title}</span>
                      <button
                        onClick={() => setMatching(row)}
                        className={clsx(
                          "flex-shrink-0 border border-[#24242a] px-1.5 text-[11px] hover:bg-[#121212]",
                          (MATCH_BADGE[row.match_status] ?? MATCH_BADGE.unmatched).tone,
                        )}
                        title="TMDb 候補を探して照合する"
                      >
                        {(MATCH_BADGE[row.match_status] ?? MATCH_BADGE.unmatched).label}・照合
                      </button>
                    </div>
                  </td>
                  <td className="py-1.5 text-gray-500">{row.year ?? "—"}</td>
                  <td className="py-1.5 pr-2 text-gray-400">
                    <ReadingInput
                      value={row.reading}
                      disabled={updateFields.isPending}
                      onCommit={(reading) => void updateWorks([row.work_id], { reading })}
                    />
                  </td>
                  <td className="py-1.5 font-mono text-xs text-gray-500">
                    {row.status === "no_country" ? (
                      <select
                        value=""
                        disabled={updateFields.isPending}
                        onChange={(e) => {
                          if (e.target.value) void updateWorks([row.work_id], { country_type: e.target.value });
                        }}
                        className="h-7 bg-[#101014] border border-amber-700/60 px-1 font-sans text-xs text-amber-300 outline-none"
                        aria-label={`${row.title} の洋邦`}
                      >
                        <option value="">洋邦を選択…</option>
                        <option value="foreign">洋画</option>
                        <option value="domestic">邦画</option>
                      </select>
                    ) : (
                      row.dest_display ?? row.note ?? "—"
                    )}
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
      {matching && (
        <CandidateDialog
          workId={matching.work_id}
          workTitle={matching.title}
          isLocked={matching.match_status === "locked"}
          onClose={() => {
            setMatching(null);
            // 照合でタイトル・年・洋邦が変わると移動先も変わるので計画を作り直す
            void qc.invalidateQueries({ queryKey: nasSortKeys.plan(folder) });
            void qc.invalidateQueries({ queryKey: workKeys.all });
          }}
        />
      )}
    </div>
  );
}
