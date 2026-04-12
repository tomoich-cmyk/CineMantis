import { useLibraryStore } from "@/store/libraryStore";
import { AppShell } from "@/components/layout/AppShell";
import { SideNav } from "@/components/layout/SideNav";
import { TopBar } from "@/components/layout/TopBar";
import { WorkList } from "@/components/works/WorkList";
import { DetailPane } from "@/components/detail/DetailPane";
import { SourcesScreen } from "@/components/sources/SourcesScreen";
import { SettingsScreen } from "@/components/settings/SettingsScreen";
import { SeriesScreen } from "@/components/series/SeriesScreen";
import { SeriesDetailScreen } from "@/components/series/SeriesDetailScreen";
import { ScanProgressBar } from "@/components/common/ScanProgressBar";
import { useThumbnailEvents } from "@/hooks/useThumbnails";
import { useMetadataEvents } from "@/hooks/useTmdb";

// ユーティリティ画面（TopBar・WorkList 非表示）
const UTILITY_SECTIONS = new Set(["sources", "settings"]);

export default function App() {
  const { activeSection, selectedWorkId, selectedSeriesId } = useLibraryStore();

  // グローバルイベントリスナー（App ライフサイクル全体で1つだけ）
  useThumbnailEvents();   // thumb:generated → TanStack Query 無効化
  useMetadataEvents();    // metadata:updated → TanStack Query 無効化

  const isUtility = UTILITY_SECTIONS.has(activeSection);
  const isSeries  = activeSection === "series";
  const detailOpen = !isUtility && selectedWorkId !== null;

  return (
    <AppShell>
      <SideNav />

      <div className="flex flex-col flex-1 min-w-0 overflow-hidden">
        {/* TopBar: ユーティリティ画面とシリーズ一覧では非表示 */}
        {!isUtility && !(isSeries && !selectedSeriesId) && <TopBar />}

        <div className="flex flex-1 min-h-0 overflow-hidden">
          {/* ユーティリティ画面 */}
          {activeSection === "sources"  && <SourcesScreen />}
          {activeSection === "settings" && <SettingsScreen />}

          {/* シリーズ一覧 */}
          {isSeries && !selectedSeriesId && <SeriesScreen />}

          {/* シリーズ詳細 */}
          {isSeries && selectedSeriesId !== null && (
            <>
              <SeriesDetailScreen seriesId={selectedSeriesId} />
              {detailOpen && <DetailPane />}
            </>
          )}

          {/* 通常のライブラリ */}
          {!isUtility && !isSeries && (
            <>
              <WorkList />
              {detailOpen && <DetailPane />}
            </>
          )}
        </div>
      </div>

      {/* グローバル進捗バー（スキャン・サムネ・メタデータバッチ） */}
      <ScanProgressBar />
    </AppShell>
  );
}
