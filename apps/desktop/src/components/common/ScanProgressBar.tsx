import { useScanProgress } from "@/hooks/useScanProgress";
import { clsx } from "clsx";

export function ScanProgressBar() {
  const progress = useScanProgress();
  if (!progress) return null;

  const pct =
    progress.phase === "done"
      ? 100
      : progress.total > 0
      ? Math.round((progress.done / progress.total) * 100)
      : 0;

  const isDone = progress.phase === "done";

  return (
    <div
      className={clsx(
        "fixed bottom-4 right-4 z-50 w-80 rounded-lg border shadow-xl transition-all",
        isDone
          ? "border-mantis-700 bg-mantis-950/90"
          : "border-surface-border bg-surface-elevated/95"
      )}
      style={{ backdropFilter: "blur(8px)" }}
    >
      <div className="px-4 py-3 flex flex-col gap-2">
        {/* Header */}
        <div className="flex items-center justify-between">
          <span className="text-xs font-medium text-gray-300">
            {isDone
              ? "スキャン完了"
              : progress.phase === "walking"
              ? "フォルダを走査中…"
              : "ファイルを解析中…"}
          </span>
          {isDone && (
            <span className="text-mantis-400 text-xs">✓</span>
          )}
        </div>

        {/* Progress bar */}
        <div className="h-1 bg-surface rounded-full overflow-hidden">
          <div
            className={clsx(
              "h-full rounded-full transition-all duration-300",
              isDone ? "bg-mantis-500" : "bg-mantis-600"
            )}
            style={{ width: `${pct}%` }}
          />
        </div>

        {/* Stats */}
        <div className="flex items-center justify-between text-xs text-gray-500">
          {isDone ? (
            <>
              <span className="text-mantis-400">
                新規 {progress.new_files} 件 / 更新 {progress.updated_files} 件
              </span>
              <span>{progress.total} ファイル</span>
            </>
          ) : (
            <>
              <span className="truncate max-w-[200px] text-gray-600">
                {progress.current_file ?? "—"}
              </span>
              <span>
                {progress.done} / {progress.total || "…"}
              </span>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
