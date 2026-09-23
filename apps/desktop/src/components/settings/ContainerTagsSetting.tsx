import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  backfillContainerTags,
  containerTagCoverage,
  type TagBackfillProgress,
  type TagBackfillResult,
} from "@/api/containerTags";

/**
 * 動画ファイルに埋め込まれたメタデータ（タイトル・年・言語など）の読み込み。
 * 新しく取り込んだファイルはスキャン時に読むので、ここは既存ファイルの後埋め用。
 * NAS 越しに全件読むと時間がかかるので、必ずユーザー操作で開始する。
 */
export function ContainerTagsSetting() {
  const qc = useQueryClient();
  const [progress, setProgress] = useState<TagBackfillProgress | null>(null);
  const [result, setResult] = useState<TagBackfillResult | null>(null);

  const { data: coverage } = useQuery({
    queryKey: ["container-tags", "coverage"],
    queryFn: containerTagCoverage,
  });

  useEffect(() => {
    const unlisten = listen<TagBackfillProgress>("tags:backfill_progress", (event) => {
      setProgress(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const { mutate: run, isPending } = useMutation({
    mutationFn: (limit?: number) => backfillContainerTags(limit),
    onSuccess: (data) => {
      setResult(data);
      setProgress(null);
      qc.invalidateQueries({ queryKey: ["container-tags"] });
      qc.invalidateQueries({ queryKey: ["works"] });
    },
  });

  const remaining = coverage ? coverage.files - coverage.with_tags : 0;

  return (
    <section className="flex flex-col gap-3 border-t border-subtle pt-6">
      <h2 className="text-sm font-medium text-gray-500">動画ファイルのメタデータ</h2>
      <p className="text-xs text-gray-600">
        ファイルに埋め込まれたタイトル・年・音声言語を読み込み、照合の手がかりに使います。
        新しく取り込むファイルはスキャン時に読み込むので、ここは既存ファイル用です。
        NAS 越しでは1件あたり2秒ほどかかります（途中でやめても、次に続きから再開します）。
      </p>

      {coverage && (
        <div className="flex flex-wrap gap-4 text-xs text-gray-600">
          <span>ファイル {coverage.files}</span>
          <span className="text-mantis-400">読み込み済み {coverage.with_tags}</span>
          <span>未読み込み {remaining}</span>
          <span>配信元あり {coverage.provider_known}</span>
          <span>経路あり {coverage.pipeline_known}</span>
          {coverage.suspicious > 0 && (
            <span className="text-amber-400">要注意 {coverage.suspicious}</span>
          )}
        </div>
      )}

      <div className="flex items-center gap-3">
        <button
          onClick={() => run(20)}
          disabled={isPending || remaining === 0}
          className="h-8 px-3 border border-[#24242a] text-sm text-gray-300 hover:text-gray-100 hover:bg-[#121212] disabled:opacity-40"
          title="まず20件だけ読み込んで様子を見ます"
        >
          20件だけ試す
        </button>
        <button
          onClick={() => run(undefined)}
          disabled={isPending || remaining === 0}
          className="h-8 px-4 bg-mantis-600 text-black text-sm font-medium hover:bg-mantis-500 disabled:opacity-40"
        >
          {isPending ? "読み込み中…" : "すべて読み込む"}
        </button>
        {progress && (
          <span className="text-xs text-gray-500">
            {progress.processed} / {progress.total}
            {progress.failed > 0 && `（失敗 ${progress.failed}）`}
          </span>
        )}
      </div>

      {result && (
        <div className="flex flex-wrap gap-4 text-xs text-gray-500">
          <span>処理 {result.processed}</span>
          <span className="text-mantis-400">メタデータあり {result.with_tags}</span>
          {result.failed > 0 && <span className="text-red-400">失敗 {result.failed}</span>}
          {result.remaining > 0 && <span>残り {result.remaining}</span>}
          {result.conflicts_created > 0 && (
            <span className="text-amber-400">
              既存の照合と食い違う作品 {result.conflicts_created} 件をレビュー対象にしました
            </span>
          )}
        </div>
      )}
    </section>
  );
}
