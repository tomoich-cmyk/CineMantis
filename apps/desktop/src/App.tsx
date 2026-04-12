import { useLibraryStore } from "@/store/libraryStore";
import { AppShell } from "@/components/layout/AppShell";
import { SideNav } from "@/components/layout/SideNav";
import { TopBar } from "@/components/layout/TopBar";
import { WorkList } from "@/components/works/WorkList";
import { DetailPane } from "@/components/detail/DetailPane";
import { SourcesScreen } from "@/components/sources/SourcesScreen";
import { ScanProgressBar } from "@/components/common/ScanProgressBar";

const SOURCES_SECTION = "sources";

export default function App() {
  const { activeSection, selectedWorkId } = useLibraryStore();
  const isSourcesView = activeSection === SOURCES_SECTION;
  const detailOpen = !isSourcesView && selectedWorkId !== null;

  return (
    <AppShell>
      <SideNav />

      <div className="flex flex-col flex-1 min-w-0 overflow-hidden">
        {!isSourcesView && <TopBar />}

        <div className="flex flex-1 min-h-0 overflow-hidden">
          {isSourcesView ? (
            <SourcesScreen />
          ) : (
            <>
              <WorkList />
              {detailOpen && <DetailPane />}
            </>
          )}
        </div>
      </div>

      {/* Global scan progress overlay */}
      <ScanProgressBar />
    </AppShell>
  );
}
