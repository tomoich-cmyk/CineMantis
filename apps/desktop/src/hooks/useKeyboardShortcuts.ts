import { useEffect, useCallback } from "react";
import { useLibraryStore } from "@/store/libraryStore";
import { useWorkList } from "@/hooks/useWorks";
import { useOpenWorkFile } from "@/hooks/useWatch";

// ─── ガード ───────────────────────────────────────────────────────────────────

/** キー入力を横取りすべきでないターゲット要素か判定 */
function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  const tag = target.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    (target as HTMLElement).isContentEditable
  );
}

/** モーダルダイアログが開いているか判定（role="dialog" を持つ要素があるか） */
function isDialogOpen(): boolean {
  return document.querySelector('[role="dialog"]') !== null;
}

// ─── フック ───────────────────────────────────────────────────────────────────

/**
 * グローバルキーボードショートカット
 *
 * Escape  … DetailPane を閉じる
 * /       … 検索欄にフォーカス（input[type="search"]）
 * Space   … 選択中の作品を再生
 * ← →    … 一覧の前後の作品に移動（選択モード中は無効）
 *
 * 以下の場合は全スキップ:
 * - 入力フィールド・select・contenteditable にフォーカス中
 * - role="dialog" が DOM に存在する（確認ダイアログ等）
 * - Ctrl / Alt / Meta キーと組み合わせ
 */
export function useKeyboardShortcuts() {
  const {
    selectedWorkId,
    setSelectedWorkId,
    isSelectMode,
  } = useLibraryStore();

  // 現在表示中の一覧順をそのまま前後移動の基準にする
  const { data: works = [] } = useWorkList();
  const { mutate: openFile } = useOpenWorkFile();

  const handler = useCallback(
    (e: KeyboardEvent) => {
      // ── ガード ──────────────────────────────────────────────────────────
      if (isEditableTarget(e.target)) return;
      if (isDialogOpen())             return;
      if (e.ctrlKey || e.altKey || e.metaKey) return;

      switch (e.key) {

        // ── Escape: DetailPane を閉じる ───────────────────────────────────
        case "Escape": {
          if (selectedWorkId !== null) {
            e.preventDefault();
            setSelectedWorkId(null);
          }
          break;
        }

        // ── /: 検索欄にフォーカス ─────────────────────────────────────────
        case "/": {
          const searchInput = document.querySelector<HTMLInputElement>(
            'input[type="search"]'
          );
          if (searchInput) {
            e.preventDefault();
            searchInput.focus();
            searchInput.select();
          }
          break;
        }

        // ── Space: 選択中の作品を再生 ─────────────────────────────────────
        case " ": {
          // 選択モード中は Space によるスクロール阻止だけ行い、再生はしない
          if (selectedWorkId !== null && !isSelectMode) {
            e.preventDefault();
            openFile(selectedWorkId);
          }
          break;
        }

        // ── ← →: 一覧の前後移動 ───────────────────────────────────────────
        case "ArrowLeft":
        case "ArrowRight": {
          // 選択モード中・一覧が空・DetailPane が閉じている時は無効
          if (isSelectMode || works.length === 0) break;

          const idx = works.findIndex((w) => w.id === selectedWorkId);

          if (idx === -1) {
            // 未選択: ArrowRight で先頭を選択
            if (e.key === "ArrowRight") {
              e.preventDefault();
              setSelectedWorkId(works[0].id);
            }
            break;
          }

          const next = e.key === "ArrowLeft" ? idx - 1 : idx + 1;
          if (next >= 0 && next < works.length) {
            e.preventDefault();
            setSelectedWorkId(works[next].id);
          }
          break;
        }
      }
    },
    [selectedWorkId, setSelectedWorkId, isSelectMode, works, openFile]
  );

  useEffect(() => {
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [handler]);
}
