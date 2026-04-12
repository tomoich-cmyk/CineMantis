import { useLibraryStore } from "@/store/libraryStore";
import { AppShell } from "@/components/layout/AppShell";
import { SideNav } from "@/components/layout/SideNav";
import { TopBar } from "@/components/layout/TopBar";
import { WorkList } from "@/components/works/WorkList";
import { DetailPane } from "@/components/detail/DetailPane";
import { SourcesScreen } from "@/components/sources/SourcesScreen";
import { SettingsScreen } from "@/components/settings/SettingsScreen";
import { ScanProgressBar } from "@/components/common/ScanProgressBar";
import { useThumbnailEvents } from "@/hooks/useThumbnails";
import { useMetadataEvents } from "@/hooks/useTmdb";

const UTILITY_SECTIONS = new Set(["sources", "settings"]);

export default function App() {
  const { activeSection, selectedWorkId } = useLibraryStore();

  // グローバルイベントリスナー（App ライフサイクル全体で1つだけ）
  useThumbnailEvents();   // thumb:generated → TanStack Query 無効化
  useMetadataEvents();    // metadata:updated → TanStack Query 無効化

  const isUtility = UTILITY_SECTIONS.has(activeSection);
  const detailOpen = !isUtility && selectedWorkId !== null;

  return (
    <AppShell>
      <SideNav />

      <div className="flex flex-col flex-1 min-w-0 overflow-hidden">
        {!isUtility && <TopBar />}

        <div className="flex flex-1 min-h-0 overflow-hidden">
          {activeSection === "sources"  && <SourcesScreen />}
          {activeSection === "settings" && <SettingsScreen />}
          {!isUtility && (
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
