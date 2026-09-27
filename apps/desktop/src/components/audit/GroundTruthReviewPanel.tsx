import { useMemo, useState } from "react";
import { clsx } from "clsx";
import {
  useGtReviewResolve,
  useGtReviewStateChange,
  useGtReviewTask,
  useGtReviewTasks,
  useGtReviewTmdbPreview,
} from "@/hooks/useGtReview";
import type { ReviewCandidate, ReviewState, ReviewTaskDetail } from "@/api/gtReview";

// ground truth レビュー（PR3 C5c.3B）。
//
// ここに出すのは **抽出時に凍結された材料だけ**。いまの works の題名も、
// rules / Jev の score・順位・判定も出さない。機械の答えを見てから人が決めると、
// その ground truth で測った精度が機械寄りに歪む。

type StateFilter = "all" | ReviewState;

const STATE_LABELS: Record<ReviewState, string> = {
  ready: "未確認",
  deferred: "保留",
  resolved: "確定済み",
};

/** 確定しようとしている内容 */
type PendingResolution =
  | { kind: "confirm"; candidate: ReviewCandidate }
  | { kind: "pickOther"; tmdbId: number; mediaType: string; title: string }
  | { kind: "none" };

function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

// ─── 凍結された証拠 ───────────────────────────────────────────────────────────

/** 凍結 snapshot から、決めた項目だけを取り出す小道具 */
function evidenceText(evidence: Record<string, unknown>, key: string): string | null {
  const value = evidence[key];
  if (typeof value === "string") return value.trim() === "" ? null : value;
  if (typeof value === "number") return String(value);
  return null;
}

/** 文字列の配列だけを読む（それ以外の形は無いものとして扱う） */
function evidenceList(source: Record<string, unknown>, key: string): string[] {
  const value = source[key];
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string" && item.trim() !== "");
}

/**
 * 埋め込みメタデータのうち、レビューで見せると決めた項目だけ。
 *
 * `cast` や `description` のような、判断に使わないと決めた値まで
 * 目に入れないようにする。未知のフィールドも自動では出さない。
 */
function embeddedRows(evidence: Record<string, unknown>): { label: string; value: string }[] {
  const embedded =
    typeof evidence.embedded === "object" && evidence.embedded !== null
      ? (evidence.embedded as Record<string, unknown>)
      : {};
  const join = (values: string[]) => (values.length === 0 ? "—" : values.join(", "));
  return [
    { label: "タグの題名", value: evidenceText(embedded, "title") ?? "—" },
    { label: "タグの年", value: evidenceText(embedded, "year") ?? "—" },
    { label: "版の表記", value: join(evidenceList(embedded, "cut_editions")) },
    { label: "音声", value: join(evidenceList(embedded, "audio_languages")) },
    { label: "字幕", value: join(evidenceList(embedded, "subtitle_languages")) },
  ];
}

/**
 * 照合時の証拠。**出す項目は仕様で決めた分だけ。**
 *
 * snapshot を丸ごと表示すると、あとから増えたフィールド（判定寄りの値が
 * 混ざるかもしれない）まで自動的に人の目に入ってしまう。blind review を
 * 保つため、ここに並べたものだけを見せる。
 */
function EvidenceView({ detail }: { detail: ReviewTaskDetail }) {
  const evidence = detail.evidence;
  const rows: { label: string; value: string }[] = [
    { label: "照合タイトル", value: evidenceText(evidence, "derived_title") ?? "—" },
    { label: "タイトルの出どころ", value: evidenceText(evidence, "title_provenance") ?? "—" },
    { label: "種別", value: evidenceText(evidence, "media_kind") ?? "—" },
    { label: "ファイル名の年", value: evidenceText(evidence, "filename_year") ?? "—" },
    { label: "推定名の年", value: evidenceText(evidence, "title_guess_year") ?? "—" },
  ];
  const files = Array.isArray(evidence.files) ? (evidence.files as unknown[]) : [];
  const embedded = embeddedRows(evidence);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-baseline gap-2">
        <h4 className="text-sm font-semibold text-gray-100">{detail.displayTitle || "（題名なし）"}</h4>
        <span className="text-[11px] text-gray-600">
          {detail.partCount} ファイル · {detail.cohort}
        </span>
      </div>

      <dl className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-1 border border-subtle rounded px-3 py-2">
        {rows.map((row) => (
          <div key={row.label} className="contents">
            <dt className="text-[11px] text-gray-600">{row.label}</dt>
            <dd className="text-[11px] text-gray-300 break-all">{row.value}</dd>
          </div>
        ))}
      </dl>

      <details className="border border-subtle rounded">
        <summary className="px-3 py-2 text-xs text-gray-400 cursor-pointer hover:text-gray-200">
          ファイル {files.length} 件 / 埋め込みメタデータ
        </summary>
        <div className="border-t border-subtle px-3 py-2 flex flex-col gap-2">
          <ul className="flex flex-col gap-0.5">
            {files.map((file, index) => {
              const row = (file ?? {}) as Record<string, unknown>;
              const name =
                typeof row.original_file_name === "string" && row.original_file_name !== ""
                  ? row.original_file_name
                  : "（元のファイル名なし）";
              const extension =
                typeof row.extension === "string" ? row.extension : null;
              return (
                <li key={index} className="text-[11px] text-gray-400 font-mono break-all">
                  {name}
                  {extension ? ` · ${extension}` : ""}
                </li>
              );
            })}
            {files.length === 0 && <li className="text-[11px] text-gray-600">—</li>}
          </ul>
          <dl className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-1">
            {embedded.map((row) => (
              <div key={row.label} className="contents">
                <dt className="text-[11px] text-gray-600">{row.label}</dt>
                <dd className="text-[11px] text-gray-300 break-all">{row.value}</dd>
              </div>
            ))}
          </dl>
        </div>
      </details>
    </div>
  );
}

// ─── 候補 ─────────────────────────────────────────────────────────────────────

/**
 * 候補の見出し。読むのは run が凍結した snapshot の項目だけ:
 * `title` / `original_title` / `year` / `original_language`。
 *
 * **年は必ず出す。** 同名のリメイクや同題の別作品は、年が無いと人にも
 * 見分けられない。
 */
function candidateLabel(candidate: ReviewCandidate): { title: string; sub: string } {
  const snapshot = candidate.tmdbSnapshot;
  const text = (key: string): string | null => {
    const value = snapshot[key];
    return typeof value === "string" && value.trim() !== "" ? value : null;
  };
  const year = typeof snapshot.year === "number" ? String(snapshot.year) : null;
  const title = text("title") ?? `TMDB ${candidate.tmdbId}`;
  const original = text("original_title");
  const language = text("original_language");
  const sub = [original, year, language].filter(Boolean).join(" · ");
  return { title, sub };
}

function CandidateRow({
  candidate,
  selected,
  onSelect,
}: {
  candidate: ReviewCandidate;
  selected: boolean;
  onSelect: () => void;
}) {
  const { title, sub } = candidateLabel(candidate);
  return (
    <button
      onClick={onSelect}
      className={clsx(
        "w-full text-left px-3 py-2 rounded border transition-colors",
        selected
          ? "border-mantis-500 bg-mantis-900/20"
          : "border-subtle bg-surface hover:bg-surface-hover",
      )}
    >
      <div className="flex items-center gap-2">
        <span className="text-xs font-medium text-gray-100 truncate">{title}</span>
        <span className="text-[10px] px-1.5 py-0.5 rounded bg-surface-elevated text-gray-500 flex-shrink-0">
          {candidate.mediaType}
        </span>
      </div>
      <p className="text-[11px] text-gray-500 truncate">
        {sub || "—"} · TMDB {candidate.tmdbId}
      </p>
    </button>
  );
}

// ─── 候補外 TMDB ID ───────────────────────────────────────────────────────────

function PickOtherForm({
  disabled,
  onConfirm,
}: {
  disabled: boolean;
  onConfirm: (input: { tmdbId: number; mediaType: string; title: string }) => void;
}) {
  const [mediaType, setMediaType] = useState("movie");
  const [tmdbId, setTmdbId] = useState("");
  const preview = useGtReviewTmdbPreview();

  // 入力が変わったら、前の確認結果は捨てる（古い作品のまま確定させない）
  const discard = () => {
    if (preview.data || preview.error) preview.reset();
  };

  const parsedId = Number.parseInt(tmdbId, 10);
  const idIsValid = Number.isInteger(parsedId) && parsedId > 0;
  const verified = preview.data;
  const matchesInput =
    verified !== undefined && verified.tmdbId === parsedId && verified.mediaType === mediaType;

  return (
    <div className="flex flex-col gap-2 border border-subtle rounded p-3">
      <p className="text-xs text-gray-400">候補に無い作品を指定する</p>
      <div className="flex gap-2">
        <select
          value={mediaType}
          onChange={(e) => {
            setMediaType(e.target.value);
            discard();
          }}
          className="text-xs bg-surface border border-subtle rounded px-2 py-1.5 text-gray-200"
        >
          <option value="movie">movie</option>
          <option value="tv">tv</option>
        </select>
        <input
          value={tmdbId}
          onChange={(e) => {
            setTmdbId(e.target.value.replace(/[^0-9]/g, ""));
            discard();
          }}
          placeholder="TMDB ID"
          inputMode="numeric"
          className="flex-1 min-w-0 text-xs bg-surface border border-subtle rounded px-2 py-1.5 text-gray-200 font-mono"
        />
        <button
          onClick={() => preview.mutate({ tmdbId: parsedId, mediaType })}
          disabled={disabled || !idIsValid || preview.isPending}
          className="text-xs px-3 py-1.5 border border-subtle rounded text-gray-300 hover:text-gray-100 hover:border-gray-500 disabled:opacity-40 disabled:hover:text-gray-300 transition-colors"
        >
          {preview.isPending ? "確認中…" : "TMDB で確認"}
        </button>
      </div>

      {preview.error != null && (
        <p className="text-xs text-red-400 break-all">{errorText(preview.error)}</p>
      )}

      {matchesInput && verified && (
        <div className="flex flex-col gap-1 border border-mantis-800/60 bg-mantis-900/10 rounded px-3 py-2">
          <p className="text-xs text-gray-100">{verified.title}</p>
          <p className="text-[11px] text-gray-500">
            {[verified.originalTitle, verified.year].filter(Boolean).join(" · ") || "—"} ·{" "}
            {verified.mediaType} {verified.tmdbId}
          </p>
          {verified.overview && (
            <p className="text-[11px] text-gray-500 line-clamp-3">{verified.overview}</p>
          )}
          <button
            onClick={() =>
              onConfirm({
                tmdbId: verified.tmdbId,
                mediaType: verified.mediaType,
                title: verified.title,
              })
            }
            disabled={disabled}
            className="self-start mt-1 text-xs px-3 py-1.5 border border-mantis-700 rounded text-mantis-300 hover:bg-mantis-900/30 disabled:opacity-40 transition-colors"
          >
            この作品で確定
          </button>
        </div>
      )}
    </div>
  );
}

// ─── 確認ダイアログ ───────────────────────────────────────────────────────────

function ConfirmDialog({
  pending,
  note,
  onNoteChange,
  onCancel,
  onProceed,
  isPending,
  error,
}: {
  pending: PendingResolution;
  note: string;
  onNoteChange: (value: string) => void;
  onCancel: () => void;
  onProceed: () => void;
  isPending: boolean;
  /** 直前の確定が失敗した理由。modal の中に出す */
  error?: unknown;
}) {
  const summary =
    pending.kind === "confirm"
      ? `候補から確定: ${candidateLabel(pending.candidate).title}（${pending.candidate.mediaType} ${pending.candidate.tmdbId}）`
      : pending.kind === "pickOther"
        ? `候補外で確定: ${pending.title}（${pending.mediaType} ${pending.tmdbId}）`
        : "TMDB に該当なしとして確定";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div role="dialog" className="bg-surface-elevated border border-subtle rounded-lg p-6 w-96 shadow-2xl">
        <h3 className="text-sm font-semibold text-gray-100 mb-2">正解として記録する</h3>
        <p className="text-xs text-gray-300 mb-3 break-all">{summary}</p>
        <p className="text-xs text-gray-500 mb-3">
          強い正解ラベルとして残ります。作品の割り当て（works）は変わりません。
        </p>
        <textarea
          value={note}
          onChange={(e) => onNoteChange(e.target.value)}
          placeholder="メモ（任意）"
          rows={2}
          className="w-full text-xs bg-surface border border-subtle rounded px-2 py-1.5 text-gray-200 mb-3"
        />
        {error != null && (
          <div className="border border-red-800/60 bg-red-900/10 rounded px-3 py-2 mb-3">
            <p className="text-xs text-red-300 font-medium mb-1">記録できませんでした</p>
            <p className="text-[11px] text-red-400/90 break-all">{errorText(error)}</p>
          </div>
        )}
        <div className="flex gap-2 justify-end">
          <button
            onClick={onCancel}
            disabled={isPending}
            className="px-3 py-1.5 text-xs border border-subtle rounded text-gray-400 hover:text-gray-200 disabled:opacity-40 transition-colors"
          >
            キャンセル
          </button>
          <button
            onClick={onProceed}
            disabled={isPending}
            className="px-3 py-1.5 text-xs bg-mantis-900/40 border border-mantis-700/60 rounded text-mantis-300 hover:bg-mantis-900/60 disabled:opacity-40 transition-colors"
          >
            {isPending ? "記録中…" : "記録する"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ─── 本体 ─────────────────────────────────────────────────────────────────────

export function GroundTruthReviewPanel() {
  const [sampleInput, setSampleInput] = useState("");
  const [sampleId, setSampleId] = useState("");
  const [filter, setFilter] = useState<StateFilter>("all");
  const [selectedTaskId, setSelectedTaskId] = useState<number | null>(null);
  const [selectedCandidateId, setSelectedCandidateId] = useState<number | null>(null);
  const [note, setNote] = useState("");
  const [pending, setPending] = useState<PendingResolution | null>(null);

  // 状態での絞り込みは **画面側でやる**。backend に filter を渡すと、
  // 除外された課題の証拠・候補が strict 検証にかからず、その課題の破損が
  // 見えないまま一覧が成立してしまう。sample 全体を fail-closed に保つため、
  // 常に未解決の全件を読む
  const list = useGtReviewTasks(sampleId);
  const tasks = useMemo(
    () => (list.data ?? []).filter((task) => filter === "all" || task.state === filter),
    [list.data, filter],
  );
  const detail = useGtReviewTask(selectedTaskId);
  const stateChange = useGtReviewStateChange();
  const resolve = useGtReviewResolve();

  const clearSelection = () => {
    setSelectedTaskId(null);
    setSelectedCandidateId(null);
    setNote("");
    setPending(null);
  };

  const loadSample = () => {
    const next = sampleInput.trim();
    clearSelection();
    setSampleId(next);
  };

  const proceed = () => {
    if (!pending || selectedTaskId === null) return;
    const trimmed = note.trim();
    const noteArg = trimmed === "" ? undefined : trimmed;
    const input =
      pending.kind === "confirm"
        ? ({
            kind: "confirm" as const,
            taskId: selectedTaskId,
            candidateId: pending.candidate.candidateId,
            note: noteArg,
          })
        : pending.kind === "pickOther"
          ? ({
              kind: "pickOther" as const,
              taskId: selectedTaskId,
              tmdbId: pending.tmdbId,
              mediaType: pending.mediaType,
              note: noteArg,
            })
          : ({ kind: "none" as const, taskId: selectedTaskId, note: noteArg });

    // 失敗しても modal は閉じない（理由を modal の中で見せ、note も残す）
    resolve.reset();
    resolve.mutate(input, { onSuccess: clearSelection });
  };

  const busy = resolve.isPending || stateChange.isPending;

  return (
    <div className="flex flex-col gap-3">
      {/* sample の指定 */}
      <div className="flex flex-wrap items-center gap-2">
        <input
          value={sampleInput}
          onChange={(e) => setSampleInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") loadSample();
          }}
          placeholder="sample_id（例: gt-dev-001）"
          className="flex-1 min-w-[12rem] text-xs bg-surface border border-subtle rounded px-2 py-1.5 text-gray-200 font-mono"
        />
        <button
          onClick={loadSample}
          disabled={sampleInput.trim() === ""}
          className="text-xs px-3 py-1.5 border border-subtle rounded text-gray-300 hover:text-gray-100 hover:border-gray-500 disabled:opacity-40 transition-colors"
        >
          読み込む
        </button>
        <div className="flex gap-0 border border-subtle rounded overflow-hidden">
          {(["all", "ready", "deferred"] as const).map((value) => (
            <button
              key={value}
              onClick={() => setFilter(value)}
              className={clsx(
                "text-xs px-3 py-1.5 transition-colors",
                filter === value
                  ? "bg-mantis-900/30 text-mantis-300"
                  : "text-gray-500 hover:text-gray-300",
              )}
            >
              {value === "all" ? "すべて" : STATE_LABELS[value]}
            </button>
          ))}
        </div>
      </div>

      {sampleId === "" ? (
        <p className="text-xs text-gray-600">
          抽出済みの sample_id を入力すると、未確定の課題を読み込みます。
        </p>
      ) : list.isLoading ? (
        <p className="text-sm text-gray-600 animate-pulse">読み込み中…</p>
      ) : list.error != null ? (
        <div className="border border-red-800/60 bg-red-900/10 rounded px-3 py-2">
          <p className="text-xs text-red-300 font-medium mb-1">課題を読み込めません</p>
          <p className="text-[11px] text-red-400/90 break-all">{errorText(list.error)}</p>
          <p className="text-[11px] text-gray-500 mt-1">
            記録の破損は件数を減らさずエラーになります。原因を直すまで、この sample は測定に使えません。
          </p>
        </div>
      ) : (
        <div className="flex gap-3 items-start">
          {/* 左: 課題一覧 */}
          <div className="w-72 flex-shrink-0 flex flex-col gap-1.5 max-h-[32rem] overflow-auto">
            <p className="text-[11px] text-gray-600">
              {tasks.length} 件
              {filter !== "all" && ` / 未解決 ${(list.data ?? []).length} 件`}
            </p>
            {tasks.length === 0 ? (
              <p className="text-xs text-gray-600">該当する課題はありません。</p>
            ) : (
              tasks.map((task) => (
                <button
                  key={task.taskId}
                  onClick={() => {
                    setSelectedTaskId(task.taskId);
                    setSelectedCandidateId(null);
                    setNote("");
                    setPending(null);
                  }}
                  className={clsx(
                    "text-left px-3 py-2 rounded border transition-colors",
                    selectedTaskId === task.taskId
                      ? "border-mantis-500 bg-mantis-900/20"
                      : "border-subtle bg-surface hover:bg-surface-hover",
                  )}
                >
                  <div className="flex items-center gap-2">
                    <span className="text-[10px] text-gray-600 font-mono">#{task.sampleRank}</span>
                    <span className="text-xs text-gray-100 truncate flex-1">
                      {task.displayTitle || "（題名なし）"}
                    </span>
                    {task.state === "deferred" && (
                      <span className="text-[10px] px-1.5 py-0.5 rounded bg-yellow-900/30 text-yellow-400">
                        保留
                      </span>
                    )}
                  </div>
                  <p className="text-[11px] text-gray-600">
                    候補 {task.candidateCount} 件 · {task.partCount} ファイル
                  </p>
                </button>
              ))
            )}
          </div>

          {/* 右: 中身 */}
          <div className="flex-1 min-w-0">
            {selectedTaskId === null ? (
              <p className="text-xs text-gray-600">左の一覧から 1 件選んでください。</p>
            ) : detail.isLoading ? (
              <p className="text-sm text-gray-600 animate-pulse">読み込み中…</p>
            ) : detail.error != null ? (
              <div className="border border-red-800/60 bg-red-900/10 rounded px-3 py-2">
                <p className="text-xs text-red-300 font-medium mb-1">この課題は開けません</p>
                <p className="text-[11px] text-red-400/90 break-all">{errorText(detail.error)}</p>
              </div>
            ) : detail.data ? (
              (() => {
                // 確定できるのは ready のときだけ。backend も同じ条件で
                // 弾くが、画面側でも押させない（二重の防壁）
                const canResolve = detail.data.state === "ready" && !busy;
                return (
              <div className="flex flex-col gap-3">
                <EvidenceView detail={detail.data} />

                <div className="flex flex-col gap-1.5">
                  <p className="text-xs text-gray-400">
                    照合時の候補（{detail.data.candidates.length} 件）
                  </p>
                  {detail.data.candidates.length === 0 ? (
                    <p className="text-xs text-gray-600">候補はありませんでした。</p>
                  ) : (
                    detail.data.candidates.map((candidate) => (
                      <CandidateRow
                        key={candidate.candidateId}
                        candidate={candidate}
                        selected={selectedCandidateId === candidate.candidateId}
                        onSelect={() => setSelectedCandidateId(candidate.candidateId)}
                      />
                    ))
                  )}
                </div>

                <div className="flex flex-wrap gap-2">
                  <button
                    onClick={() => {
                      const candidate = detail.data?.candidates.find(
                        (c) => c.candidateId === selectedCandidateId,
                      );
                      if (candidate) setPending({ kind: "confirm", candidate });
                    }}
                    disabled={!canResolve || selectedCandidateId === null}
                    className="text-xs px-3 py-1.5 border border-mantis-700 rounded text-mantis-300 hover:bg-mantis-900/30 disabled:opacity-40 transition-colors"
                  >
                    選んだ候補で確定
                  </button>
                  <button
                    onClick={() => setPending({ kind: "none" })}
                    disabled={!canResolve}
                    className="text-xs px-3 py-1.5 border border-subtle rounded text-gray-300 hover:text-gray-100 hover:border-gray-500 disabled:opacity-40 transition-colors"
                  >
                    該当なし
                  </button>
                  {detail.data.state === "ready" ? (
                    <button
                      onClick={() =>
                        stateChange.mutate({ taskId: detail.data!.taskId, next: "defer" })
                      }
                      disabled={busy}
                      className="text-xs px-3 py-1.5 border border-subtle rounded text-gray-400 hover:text-gray-200 disabled:opacity-40 transition-colors"
                    >
                      後で見る
                    </button>
                  ) : (
                    <button
                      onClick={() =>
                        stateChange.mutate({ taskId: detail.data!.taskId, next: "resume" })
                      }
                      disabled={busy}
                      className="text-xs px-3 py-1.5 border border-subtle rounded text-gray-400 hover:text-gray-200 disabled:opacity-40 transition-colors"
                    >
                      保留を解除
                    </button>
                  )}
                </div>

                {detail.data.state === "deferred" && (
                  <p className="text-[11px] text-yellow-500/90">
                    保留中の課題は確定できません。先に保留を解除してください。
                  </p>
                )}

                <PickOtherForm
                  // task ごとに作り直す。前の課題で確認した TMDB 作品が
                  // 次の課題に残ると、正しい作品を **別の課題へ**付けてしまう
                  key={detail.data.taskId}
                  disabled={!canResolve}
                  onConfirm={(input) => setPending({ kind: "pickOther", ...input })}
                />

                {resolve.error != null && (
                  <p className="text-xs text-red-400 break-all">{errorText(resolve.error)}</p>
                )}
                {stateChange.error != null && (
                  <p className="text-xs text-red-400 break-all">{errorText(stateChange.error)}</p>
                )}
              </div>
                );
              })()
            ) : null}
          </div>
        </div>
      )}

      {pending && (
        <ConfirmDialog
          pending={pending}
          note={note}
          onNoteChange={setNote}
          onCancel={() => {
            resolve.reset();
            setPending(null);
          }}
          onProceed={proceed}
          isPending={resolve.isPending}
          error={resolve.error}
        />
      )}
    </div>
  );
}
