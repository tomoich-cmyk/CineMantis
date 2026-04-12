import { useScanProgress, useThumbBatchProgress, useMetadataBatchProgress } from "@/hooks/useScanProgress";
import { clsx } from "clsx";

function ProgressToast({
  label,
  sub,
  pct,
  isDone,
  accent,
}: {
  label: string;
  sub: string;
  pct: number;
  isDone: boolean;
  accent: "mantis" | "blue" | "purple";
}) {
  const barColor =
    accent === "mantis" ? "bg-mantis-500"
    : accent === "purple" ? "bg-purple-500"
    : "bg-blue-500";
  const borderColor = isDone
    ? accent === "mantis" ? "border-mantis-700"
      : accent === "purple" ? "border-purple-700"
      : "border-blue-700"
    : "border-surface-border";
  const bgColor = isDone
    ? accent === "mantis" ? "bg-mantis-950/90"
      : accent === "purple" ? "bg-purple-950/90"
      : "bg-blue-950/90"
    : "bg-surface-elevated/95";

  return (
    <div
      className={clsx(
        "w-80 rounded-lg border shadow-xl transition-all",
        borderColor, bgColor
      )}
      style={{ backdropFilter: "blur(8px)" }}
    >
      <div className="px-4 py-3 flex flex-col gap-2">
        <div className="flex items-center justify-between">
          <span className="text-xs font-medium text-gray-300">{label}</span>
          {isDone && (
            <span className={accent === "mantis" ? "text-mantis-400 text-xs" : accent === "purple" ? "text-purple-400 text-xs" : "text-blue-400 text-xs"}>
              ✓
            </span>
          )}
        </div>
        <div className="h-1 bg-surface rounded-full overflow-hidden">
          <div
            className={clsx("h-full rounded-full transition-all duration-300", barColor)}
            style={{ width: `${pct}%` }}
          />
        </div>
        <div className="flex items-center justify-between text-xs text-gray-500">
          <span className="truncate max-w-[200px] text-gray-600">{sub}</span>
          <span>{pct}%</span>
        </div>
      </div>
    </div>
  );
}

export function ScanProgressBar() {
  const scan = useScanProgress();
  const thumb = useThumbBatchProgress();
  const meta = useMetadataBatchProgress();

  if (!scan && !thumb && !meta) return null;

  return (
    <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2 items-end">
      {scan && (
        <ProgressToast
          label={
            scan.phase === "done" ? "スキャン完了"
            : scan.phase === "walking" ? "フォルダを走査中…"
            : "ファイルを解析中…"
          }
          sub={
            scan.phase === "done"
              ? `新規 ${scan.new_files} 件 / 更新 ${scan.updated_files} 件`
              : scan.current_file ?? "—"
          }
          pct={
            scan.phase === "done" ? 100
            : scan.total > 0 ? Math.round((scan.done / scan.total) * 100)
            : 0
          }
          isDone={scan.phase === "done"}
          accent="mantis"
        />
      )}

      {thumb && thumb.total > 0 && (
        <ProgressToast
          label={
            thumb.done + thumb.failed >= thumb.total
              ? "サムネイル生成完了"
              : "サムネイルを生成中…"
          }
          sub={
            thumb.done + thumb.failed >= thumb.total
              ? `成功 ${thumb.done} 件 / 失敗 ${thumb.failed} 件`
              : `${thumb.done} / ${thumb.total}`
          }
          pct={
            thumb.total > 0
              ? Math.round(((thumb.done + thumb.failed) / thumb.total) * 100)
              : 0
          }
          isDone={thumb.done + thumb.failed >= thumb.total}
          accent="blue"
        />
      )}

      {meta && meta.total > 0 && (
        <ProgressToast
          label={
            meta.processed >= meta.total
              ? "メタデータ照合完了"
              : "メタデータを照合中…"
          }
          sub={
            meta.processed >= meta.total
              ? `照合成功 ${meta.matched} 件 / スキップ ${meta.skipped} 件 / 失敗 ${meta.failed} 件`
              : `${meta.processed} / ${meta.total}`
          }
          pct={
            meta.total > 0
              ? Math.round((meta.processed / meta.total) * 100)
              : 0
          }
          isDone={meta.processed >= meta.total}
          accent="purple"
        />
      )}
    </div>
  );
}
