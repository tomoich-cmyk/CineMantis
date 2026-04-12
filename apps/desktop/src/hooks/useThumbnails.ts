import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { useQueryClient } from "@tanstack/react-query";
import { workKeys } from "./useWorks";

interface ThumbGeneratedEvent {
  work_id: number;
  thumb_path: string;
}

/**
 * `thumb:generated` イベントをグローバルに購読する。
 * サムネイルが生成されたらその work の detail クエリを無効化して
 * WorkCard / DetailPane が再レンダリングされる。
 *
 * App.tsx トップレベルで一度だけマウントする。
 */
export function useThumbnailEvents() {
  const qc = useQueryClient();
  // 不要な重複 listen を避けるための ref
  const mounted = useRef(false);

  useEffect(() => {
    if (mounted.current) return;
    mounted.current = true;

    let unlisten: (() => void) | undefined;

    listen<ThumbGeneratedEvent>("thumb:generated", (event) => {
      const { work_id } = event.payload;
      // detail キャッシュを更新して thumb_path を反映
      qc.invalidateQueries({ queryKey: workKeys.detail(work_id) });
      // list も更新（グリッドの thumb 表示に反映）
      qc.invalidateQueries({ queryKey: workKeys.all });
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      unlisten?.();
      mounted.current = false;
    };
  }, [qc]);
}

/**
 * バッチ進捗を購読して thumb:batch_progress を useScanProgress と同様に扱う。
 * SourcesScreen などから個別に使う。
 */
export { useScanProgress as useThumbBatchProgress } from "./useScanProgress";
