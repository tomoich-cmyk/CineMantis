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
import { PersonsScreen } from "@/components/persons/PersonsScreen";
import { PersonDetailScreen } from "@/components/persons/PersonDetailScreen";
import { AttentionCenterScreen } from "@/components/attention/AttentionCenterScreen";
import { BackupScreen } from "@/components/backup/BackupScreen";
import { AuditScreen } from "@/components/audit/AuditScreen";
import { AboutScreen } from "@/components/about/AboutScreen";
import { ScanProgressBar } from "@/components/common/ScanProgressBar";
import { useThumbnailEvents } from "@/hooks/useThumbnails";
import { useMetadataEvents } from "@/hooks/useTmdb";
import { useKeyboardShortcuts } from "@/hooks/useKeyboardShortcuts";

// ユーティリティ画面（TopBar・WorkList 非表示）
const UTILITY_SECTIONS = new Set(["sources", "settings", "backup", "needs-attention", "audit", "about"]);

export default function App() {
  const { activeSection, selectedWorkId, selectedSeriesId, selectedPersonId } = useLibraryStore();

  // グローバルイベントリスナー・ショートカット
  useThumbnailEvents();
  useMetadataEvents();
  useKeyboardShortcuts();

  const isUtility       = UTILITY_SECTIONS.has(activeSection);
  const isSeries        = activeSection === "series";
  const isPersons       = activeSection === "persons";
  const isNeedsAttention = activeSection === "needs-attention";
  const isBackup        = activeSection === "backup";
  const detailOpen      = !isUtility && selectedWorkId !== null;

  return (
    <AppShell>
      <SideNav />

      <div className="flex flex-col flex-1 min-w-0 overflow-hidden">
        {/* TopBar: ユーティリティ・シリーズ一覧・人物一覧では非表示 */}
        {!isUtility
          && !(isSeries  && !selectedSeriesId)
          && !(isPersons && !selectedPersonId)
          && <TopBar />}

        <div className="flex flex-1 min-h-0 overflow-hidden">
          {/* ユーティリティ画面 */}
          {activeSection === "sources"  && <SourcesScreen />}
          {activeSection === "settings" && <SettingsScreen />}
          {isBackup                     && <BackupScreen />}
          {isNeedsAttention             && <AttentionCenterScreen />}
          {activeSection === "audit"    && <AuditScreen />}
          {activeSection === "about"    && <AboutScreen />}

          {/* シリーズ一覧 */}
          {isSeries && !selectedSeriesId && <SeriesScreen />}

          {/* シリーズ詳細 */}
          {isSeries && selectedSeriesId !== null && (
            <>
              <SeriesDetailScreen seriesId={selectedSeriesId} />
              {detailOpen && <DetailPane />}
            </>
          )}

          {/* 人物一覧 */}
          {isPersons && !selectedPersonId && <PersonsScreen />}

          {/* 人物詳細 */}
          {isPersons && selectedPersonId !== null && (
            <>
              <PersonDetailScreen personId={selectedPersonId} />
              {detailOpen && <DetailPane />}
            </>
          )}

          {/* 通常のライブラリ（スマートコレクション含む） */}
          {!isUtility && !isSeries && !isPersons && (
            <>
              <WorkList />
              {detailOpen && <DetailPane />}
            </>
          )}
        </div>
      </div>

      {/* グローバル進捗バー */}
      <ScanProgressBar />
    </AppShell>
  );
}
