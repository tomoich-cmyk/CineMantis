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
import { AwardsScreen } from "@/components/awards/AwardsScreen";
import { AwardDetailScreen } from "@/components/awards/AwardDetailScreen";
import { AttentionCenterScreen } from "@/components/attention/AttentionCenterScreen";
import { BackupScreen } from "@/components/backup/BackupScreen";
import { AuditScreen } from "@/components/audit/AuditScreen";
import { AboutScreen } from "@/components/about/AboutScreen";
import { ScanProgressBar } from "@/components/common/ScanProgressBar";
import { useThumbnailEvents } from "@/hooks/useThumbnails";
import { useMetadataEvents } from "@/hooks/useTmdb";
import { useKeyboardShortcuts } from "@/hooks/useKeyboardShortcuts";

const UTILITY_SECTIONS = new Set(["sources", "settings", "backup", "needs-attention", "audit", "about"]);

export default function App() {
  const {
    activeSection,
    selectedWorkId,
    selectedSeriesId,
    selectedPersonId,
    selectedAwardBodyId,
  } = useLibraryStore();

  useThumbnailEvents();
  useMetadataEvents();
  useKeyboardShortcuts();

  const isUtility = UTILITY_SECTIONS.has(activeSection);
  const isSeries = activeSection === "series";
  const isPersons = activeSection === "persons";
  const isAwards = activeSection === "awards";
  const isBackup = activeSection === "backup";
  const isNeedsAttention = activeSection === "needs-attention";
  const detailOpen = !isUtility && selectedWorkId !== null;
  const showTopBar =
    !isUtility &&
    !isAwards &&
    !(isSeries && !selectedSeriesId) &&
    !(isPersons && !selectedPersonId);

  return (
    <AppShell>
      <SideNav />

      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        {showTopBar && <TopBar />}

        <div className="flex min-h-0 flex-1 overflow-hidden">
          {activeSection === "sources" && <SourcesScreen />}
          {activeSection === "settings" && <SettingsScreen />}
          {isBackup && <BackupScreen />}
          {isNeedsAttention && <AttentionCenterScreen />}
          {activeSection === "audit" && <AuditScreen />}
          {activeSection === "about" && <AboutScreen />}

          {isSeries && !selectedSeriesId && <SeriesScreen />}
          {isSeries && selectedSeriesId !== null && (
            <>
              <SeriesDetailScreen seriesId={selectedSeriesId} />
              {detailOpen && <DetailPane />}
            </>
          )}

          {isPersons && !selectedPersonId && <PersonsScreen />}
          {isPersons && selectedPersonId !== null && (
            <>
              <PersonDetailScreen personId={selectedPersonId} />
              {detailOpen && <DetailPane />}
            </>
          )}

          {isAwards && !selectedAwardBodyId && <AwardsScreen />}
          {isAwards && selectedAwardBodyId !== null && (
            <AwardDetailScreen awardBodyId={selectedAwardBodyId} />
          )}

          {!isUtility && !isSeries && !isPersons && !isAwards && (
            <>
              <WorkList />
              {detailOpen && <DetailPane />}
            </>
          )}
        </div>
      </div>

      <ScanProgressBar />
    </AppShell>
  );
}
