import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useLibraryStore } from "@/store/libraryStore";
import { useWorkDetail, useWorkTags, useUpdateStats } from "@/hooks/useWorks";
import { useAutoMatchWork, useClearTmdbMatch, useRefreshTmdbMetadata, useUnlockTmdbMatch } from "@/hooks/useTmdb";
import { useWorkPersons } from "@/hooks/usePersons";
import { useOpenWorkFile, useSetWatchStatus, useUpdateResumePosition } from "@/hooks/useWatch";
import { StarRating } from "@/components/common/StarRating";
import { TagBadge } from "@/components/common/TagBadge";
import { CandidateDialog } from "@/components/tmdb/CandidateDialog";

const ROLE_LABELS: Record<string, string> = {
  director: "監督",
  writer:   "脚本",
  cast:     "出演",
};

// ─── ユーティリティ ───────────────────────────────────────────────────────────

function toAssetUrl(path: string): string {
  return convertFileSrc(path);
}

function WorkImage({ src, alt }: { src: string | null; alt: string }) {
  const [failed, setFailed] = useState(false);
  if (!src || failed) {
    return (
      <div className="w-full h-full flex items-center justify-center bg-surface">
        <span className="text-5xl opacity-20">🎬</span>
      </div>
    );
  }
  return (
    <img
      src={toAssetUrl(src)}
      alt={alt}
      className="w-full h-full object-cover rounded"
      onError={() => setFailed(true)}
    />
  );
}

function formatRuntime(sec: number | null) {
  if (!sec) return null;
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  return h > 0 ? `${h}時間${m}分` : `${m}分`;
}

function formatPosition(sec: number): string {
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const s = Math.floor(sec % 60);
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  return `${m}:${String(s).padStart(2, "0")}`;
}

const MATCH_LABELS: Record<string, { label: string; cls: string }> = {
  matched:  { label: "照合済",   cls: "text-mantis-500" },
  auto:     { label: "自動照合", cls: "text-mantis-500" },
  manual:   { label: "手動照合", cls: "text-blue-400"   },
  locked:   { label: "固定",     cls: "text-yellow-400" },
  pending:  { label: "試行済",   cls: "text-orange-400" },
  unmatched:{ label: "未照合",   cls: "text-gray-600"   },
};

type WatchStatus = "unwatched" | "watching" | "watched" | "skipped";

const WATCH_PILLS: { value: WatchStatus; label: string; cls: string }[] = [
  { value: "unwatched", label: "未視聴", cls: "border-gray-700 text-gray-500 hover:text-gray-300" },
  { value: "watching",  label: "視聴中", cls: "border-blue-800/60 text-blue-400 hover:bg-blue-900/20" },
  { value: "watched",   label: "視聴済", cls: "border-mantis-700/60 text-mantis-400 hover:bg-mantis-900/20" },
  { value: "skipped",   label: "スキップ", cls: "border-gray-700 text-gray-600 hover:text-gray-400" },
];

// ─── コンポーネント ───────────────────────────────────────────────────────────

export function DetailPane() {
  const { selectedWorkId, setSelectedWorkId, setActiveSection, setSelectedPersonId } = useLibraryStore();
  const { data: work, isLoading } = useWorkDetail(selectedWorkId);
  const { data: tags = [] } = useWorkTags(selectedWorkId);
  const { data: persons = [] } = useWorkPersons(selectedWorkId);
  const { mutate: updateStats } = useUpdateStats();
  const { mutate: autoMatch, isPending: autoMatching } = useAutoMatchWork();
  const { mutate: clearMatch } = useClearTmdbMatch();
  const { mutate: refreshMeta, isPending: refreshing } = useRefreshTmdbMetadata();
  const { mutate: unlockMatch, isPending: unlocking } = useUnlockTmdbMatch();
  const { mutate: openFile, isPending: opening } = useOpenWorkFile();
  const { mutate: setStatus } = useSetWatchStatus();
  const { mutate: savePosition } = useUpdateResumePosition();
  const [showCandidates, setShowCandidates] = useState(false);
  const [positionInput, setPositionInput] = useState("");

  function navigateToPerson(personId: number) {
    setSelectedPersonId(personId);
    setActiveSection("persons");
  }

  if (!selectedWorkId) return null;

  if (isLoading) {
    return (
      <aside className="w-72 flex-shrink-0 flex flex-col border-l border-subtle bg-surface-elevated items-center justify-center">
        <span className="text-gray-600 animate-pulse text-sm">読み込み中…</span>
      </aside>
    );
  }

  if (!work) return null;

  const matchInfo = MATCH_LABELS[work.match_status] ?? MATCH_LABELS.unmatched;
  const isMatched = ["auto", "manual", "locked", "matched"].includes(work.match_status);
  const isPending = work.match_status === "pending";
  const isLocked  = work.match_status === "locked";

  // 再生進捗
  const progress =
    work.resume_position_sec !== null && work.runtime_sec !== null && work.runtime_sec > 0
      ? Math.min(work.resume_position_sec / work.runtime_sec, 1)
      : null;

  return (
    <>
      <aside className="w-72 flex-shrink-0 flex flex-col border-l border-subtle bg-surface-elevated overflow-y-auto">

        {/* ── ヘッダー ── */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-subtle flex-shrink-0">
          <span className="text-xs text-gray-400 font-medium uppercase tracking-wider">詳細</span>
          <button
            onClick={() => setSelectedWorkId(null)}
            className="text-gray-600 hover:text-gray-300 text-lg leading-none transition-colors"
          >
            ×
          </button>
        </div>

        {/* ── ポスター / サムネイル ── */}
        <div className="mx-4 mt-4 aspect-[2/3] bg-surface rounded overflow-hidden flex-shrink-0 relative group/poster">
          <WorkImage
            src={work.poster_path ?? work.thumb_path}
            alt={work.title}
          />
          {/* ポスター上の ▶ オーバーレイ */}
          <button
            onClick={() => openFile(work.id)}
            disabled={opening}
            className="absolute inset-0 flex items-center justify-center
                       opacity-0 group-hover/poster:opacity-100 transition-opacity bg-black/30"
          >
            <span className="w-12 h-12 flex items-center justify-center rounded-full bg-black/70 text-white text-2xl hover:scale-110 transition-transform">
              {opening ? "…" : "▶"}
            </span>
          </button>
        </div>

        {/* ── 再生ボタン ── */}
        <div className="px-4 pt-3">
          <button
            onClick={() => openFile(work.id)}
            disabled={opening}
            className="w-full py-2 flex items-center justify-center gap-2 text-sm font-medium
                       bg-mantis-700/30 border border-mantis-700/60 text-mantis-300 rounded
                       hover:bg-mantis-700/50 transition-colors disabled:opacity-40"
          >
            <span>{opening ? "…" : "▶"}</span>
            <span>{opening ? "起動中…" : work.resume_position_sec ? "続きから再生" : "再生"}</span>
          </button>
        </div>

        {/* ── 再生進捗 / 再開位置インジケータ ── */}
        {progress !== null && work.resume_position_sec !== null && (
          <div className="px-4 pt-2">
            <div className="w-full h-1 bg-surface rounded-full overflow-hidden">
              <div
                className="h-full bg-blue-500 rounded-full transition-all"
                style={{ width: `${(progress * 100).toFixed(1)}%` }}
              />
            </div>
            <div className="flex items-center justify-between mt-1 text-[11px] text-gray-600">
              <span>{formatPosition(work.resume_position_sec)}</span>
              <span>{formatRuntime(work.runtime_sec)}</span>
            </div>
            {/* 再開位置を手動で更新するフォーム */}
            <div className="flex items-center gap-1 mt-1.5">
              <input
                type="text"
                value={positionInput}
                onChange={(e) => setPositionInput(e.target.value)}
                placeholder="位置(秒)を入力"
                className="flex-1 bg-surface border border-subtle rounded px-2 py-0.5 text-[11px] text-gray-400 placeholder-gray-700 outline-none focus:border-mantis-600"
              />
              <button
                onClick={() => {
                  const sec = parseFloat(positionInput);
                  if (!isNaN(sec) && sec >= 0) {
                    savePosition({ workId: work.id, positionSec: sec });
                    setPositionInput("");
                  }
                }}
                className="text-[11px] px-2 py-0.5 border border-subtle rounded text-gray-600 hover:text-gray-300 transition-colors"
              >
                更新
              </button>
            </div>
          </div>
        )}

        {/* ── タイトル / 年 / 尺 ── */}
        <div className="px-4 py-3 flex flex-col gap-1">
          <h2 className="text-base font-semibold text-gray-100 leading-snug">
            {work.title}
          </h2>
          {work.original_title && work.original_title !== work.title && (
            <p className="text-xs text-gray-500 italic">{work.original_title}</p>
          )}
          <div className="flex items-center gap-2 text-xs text-gray-500 flex-wrap">
            {work.year         && <span>{work.year}</span>}
            {work.runtime_sec  && <><span>·</span><span>{formatRuntime(work.runtime_sec)}</span></>}
            {work.external_rating && (
              <><span>·</span><span className="text-yellow-500">★ {work.external_rating.toFixed(1)}</span></>
            )}
          </div>

          {/* ジャンル */}
          {work.genres.length > 0 && (
            <div className="flex flex-wrap gap-1 mt-1">
              {work.genres.map((g) => (
                <span key={g} className="text-xs px-1.5 py-0.5 bg-surface rounded text-gray-500">
                  {g}
                </span>
              ))}
            </div>
          )}
        </div>

        {/* ── あらすじ ── */}
        {work.synopsis && (
          <div className="px-4 py-2 border-t border-subtle">
            <p className="text-xs text-gray-400 leading-relaxed line-clamp-6">
              {work.synopsis}
            </p>
          </div>
        )}

        {/* ── ユーザー統計 ── */}
        <div className="px-4 py-2 border-t border-subtle">
          <div className="flex flex-col gap-2.5">
            {/* 評価 */}
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-500">マイ評価</span>
              <StarRating
                value={work.user_rating}
                size="sm"
                onChange={(v) => updateStats({ work_id: work.id, user_rating: v })}
              />
            </div>

            {/* 視聴状態 — クイックピル */}
            <div className="flex flex-col gap-1">
              <span className="text-xs text-gray-500">視聴状態</span>
              <div className="flex gap-1 flex-wrap">
                {WATCH_PILLS.map((pill) => (
                  <button
                    key={pill.value}
                    onClick={() => setStatus({ workId: work.id, status: pill.value })}
                    className={`px-2.5 py-0.5 text-xs rounded-full border transition-colors ${
                      work.watch_status === pill.value
                        ? pill.cls + " opacity-100 ring-1 ring-current/30"
                        : "border-transparent text-gray-700 hover:border-gray-700 hover:text-gray-500"
                    }`}
                  >
                    {pill.label}
                  </button>
                ))}
              </div>
            </div>

            {/* お気に入り */}
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-500">お気に入り</span>
              <button
                onClick={() => updateStats({ work_id: work.id, is_favorite: !work.is_favorite })}
                className={`text-lg transition-transform hover:scale-110 ${work.is_favorite ? "text-yellow-400" : "text-gray-700"}`}
              >
                ★
              </button>
            </div>

            {/* 視聴回数 */}
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-500">視聴回数</span>
              <span className="text-xs text-gray-400">{work.play_count} 回</span>
            </div>
            {work.last_played_at && (
              <div className="flex items-center justify-between">
                <span className="text-xs text-gray-500">最終視聴</span>
                <span className="text-xs text-gray-500">
                  {new Date(work.last_played_at).toLocaleDateString("ja-JP")}
                </span>
              </div>
            )}
          </div>
        </div>

        {/* ── タグ ── */}
        {tags.length > 0 && (
          <div className="px-4 py-2 border-t border-subtle">
            <div className="flex flex-wrap gap-1">
              {tags.map((tag) => <TagBadge key={tag.id} tag={tag} />)}
            </div>
          </div>
        )}

        {/* ── 人物（監督・脚本・出演） ── */}
        {persons.length > 0 && (
          <div className="px-4 py-2 border-t border-subtle">
            {(["director", "writer", "cast"] as const).map((role) => {
              const group = persons.filter((p) => p.role === role);
              if (group.length === 0) return null;
              return (
                <div key={role} className="flex items-start gap-2 mb-1.5">
                  <span className="text-[10px] text-gray-600 w-8 pt-0.5 flex-shrink-0">
                    {ROLE_LABELS[role]}
                  </span>
                  <div className="flex flex-wrap gap-1">
                    {group.slice(0, role === "cast" ? 8 : 5).map((p) => (
                      <button
                        key={p.person_id}
                        onClick={() => navigateToPerson(p.person_id)}
                        className="text-[11px] text-gray-400 hover:text-mantis-400 transition-colors underline-offset-2 hover:underline"
                        title={p.character_name ?? p.name}
                      >
                        {p.name}
                      </button>
                    ))}
                    {role === "cast" && group.length > 8 && (
                      <span className="text-[11px] text-gray-700">+{group.length - 8}</span>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}

        {/* ── メモ ── */}
        <div className="px-4 py-3 border-t border-subtle">
          <textarea
            placeholder="メモ…"
            value={work.personal_note ?? ""}
            onChange={(e) => updateStats({ work_id: work.id, personal_note: e.target.value })}
            rows={3}
            className="w-full bg-surface border border-subtle rounded px-2 py-1.5 text-xs text-gray-300 placeholder-gray-700 outline-none focus:border-mantis-600 resize-none"
          />
        </div>

        {/* ── 外部情報 / 照合 ── */}
        <div className="px-4 py-3 border-t border-subtle mt-auto">
          {/* 照合状態 */}
          <div className="flex items-center justify-between text-xs mb-2">
            <span className="text-gray-500">照合状態</span>
            <div className="flex items-center gap-1.5">
              {isLocked && <span className="text-yellow-500 text-[10px]">🔒</span>}
              <span className={matchInfo.cls}>{matchInfo.label}</span>
              {work.match_confidence !== null && isMatched && (
                <span className="text-gray-600 font-mono">{work.match_confidence}%</span>
              )}
            </div>
          </div>

          {/* TMDb ID */}
          {work.tmdb_id && (
            <div className="flex items-center justify-between text-xs mb-3">
              <span className="text-gray-600">TMDb ID</span>
              <a
                href={`https://www.themoviedb.org/${work.tmdb_media_type ?? "movie"}/${work.tmdb_id}`}
                target="_blank"
                rel="noopener noreferrer"
                className="text-blue-500 hover:text-blue-400 font-mono"
              >
                {work.tmdb_id}
              </a>
            </div>
          )}

          {/* アクションボタン */}
          <div className="flex flex-col gap-1.5">
            {/* 試行済ヒント */}
            {isPending && (
              <p className="text-[11px] text-orange-400/70 leading-relaxed">
                一括照合で候補が見つかりませんでした。「候補を探す」で別のキーワードを試すか、「自動照合」で再試行できます。
              </p>
            )}

            {/* 候補を探す（locked でも使用可） */}
            <button
              onClick={() => setShowCandidates(true)}
              className="w-full py-1.5 text-xs border border-subtle rounded text-gray-400 hover:text-gray-200 hover:border-gray-500 transition-colors"
            >
              候補を探す
            </button>

            {/* 自動照合（未照合・試行済のみ・locked 時は非表示） */}
            {!isMatched && !isLocked && (
              <button
                onClick={() => autoMatch(work.id)}
                disabled={autoMatching}
                className="w-full py-1.5 text-xs border border-mantis-800 rounded text-mantis-400 hover:bg-mantis-900/30 transition-colors disabled:opacity-30"
              >
                {autoMatching ? "照合中…" : isPending ? "自動照合を再試行" : "自動照合"}
              </button>
            )}

            {/* 再取得（照合済・locked 以外） */}
            {isMatched && !isLocked && (
              <button
                onClick={() => refreshMeta(work.id)}
                disabled={refreshing}
                className="w-full py-1.5 text-xs border border-subtle rounded text-gray-500 hover:text-gray-300 transition-colors disabled:opacity-30"
              >
                {refreshing ? "更新中…" : "メタデータ再取得"}
              </button>
            )}

            {/* 固定解除（locked のみ） */}
            {isLocked && (
              <button
                onClick={() => unlockMatch(work.id)}
                disabled={unlocking}
                className="w-full py-1.5 text-xs border border-yellow-800/60 rounded text-yellow-600 hover:text-yellow-400 hover:border-yellow-600 transition-colors disabled:opacity-30"
              >
                {unlocking ? "解除中…" : "🔓 固定を解除"}
              </button>
            )}

            {/* 照合解除（照合済・locked 以外） */}
            {isMatched && !isLocked && (
              <button
                onClick={() => clearMatch(work.id)}
                className="w-full py-1.5 text-xs text-gray-700 hover:text-gray-500 transition-colors"
              >
                照合を解除
              </button>
            )}
          </div>
        </div>
      </aside>

      {/* 候補ダイアログ */}
      {showCandidates && (
        <CandidateDialog
          workId={work.id}
          workTitle={work.title}
          isLocked={isLocked}
          onClose={() => setShowCandidates(false)}
        />
      )}
    </>
  );
}
