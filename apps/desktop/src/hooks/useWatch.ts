import { useMutation, useQueryClient } from "@tanstack/react-query";
import { openWorkFile, setWatchStatus, updateResumePosition } from "@/api/watch";
import { workKeys } from "@/hooks/useWorks";

// ── ファイルを開く ─────────────────────────────────────────────────────────────

export function useOpenWorkFile() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (workId: number) => openWorkFile(workId),
    onSuccess: (_data, workId) => {
      // play_count / watch_status が更新されるので両方 invalidate
      qc.invalidateQueries({ queryKey: workKeys.detail(workId) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    },
  });
}

// ── 視聴状態の変更 ────────────────────────────────────────────────────────────

export function useSetWatchStatus() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      workId,
      status,
    }: {
      workId: number;
      status: "unwatched" | "watching" | "watched" | "skipped";
    }) => setWatchStatus(workId, status),
    onSuccess: (_data, { workId }) => {
      qc.invalidateQueries({ queryKey: workKeys.detail(workId) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    },
  });
}

// ── 再生位置の保存 ────────────────────────────────────────────────────────────

export function useUpdateResumePosition() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      workId,
      positionSec,
    }: {
      workId: number;
      positionSec: number;
    }) => updateResumePosition(workId, positionSec),
    onSuccess: (_data, { workId }) => {
      qc.invalidateQueries({ queryKey: workKeys.detail(workId) });
    },
  });
}
