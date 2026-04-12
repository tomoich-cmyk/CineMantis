import { useLibraryStore } from "@/store/libraryStore";
import type { NavSection } from "@cinemantis/shared-types";
import { clsx } from "clsx";

interface NavItem {
  id: NavSection;
  label: string;
  icon: string;
  group?: string;
}

const NAV_ITEMS: NavItem[] = [
  // Library
  { id: "all-movies", label: "映画", icon: "🎬", group: "ライブラリ" },
  { id: "all-drama", label: "ドラマ", icon: "📺", group: "ライブラリ" },
  { id: "series", label: "シリーズ", icon: "🗂", group: "ライブラリ" },
  { id: "persons", label: "人物", icon: "👤", group: "ライブラリ" },
  { id: "tags", label: "タグ", icon: "🏷", group: "ライブラリ" },
  // Smart lists
  { id: "unwatched", label: "未視聴", icon: "⬜", group: "スマート" },
  { id: "watching", label: "視聴中", icon: "▶", group: "スマート" },
  { id: "recently-added", label: "最近追加", icon: "🆕", group: "スマート" },
  { id: "high-rated", label: "高評価", icon: "⭐", group: "スマート" },
];

const GROUPS = ["ライブラリ", "スマート"];

export function SideNav() {
  const { activeSection, setActiveSection } = useLibraryStore();

  return (
    <nav className="w-48 flex-shrink-0 flex flex-col bg-surface-elevated border-r border-subtle overflow-y-auto">
      {/* Logo */}
      <div className="flex items-center gap-2 px-4 py-4 border-b border-subtle">
        <span className="text-mantis-500 font-bold text-base tracking-tight">CineMantis</span>
      </div>

      {/* Nav sections */}
      <div className="flex-1 py-2">
        {GROUPS.map((group) => (
          <div key={group} className="mb-1">
            <div className="px-4 pt-3 pb-1 text-xs font-semibold text-gray-500 uppercase tracking-wider">
              {group}
            </div>
            {NAV_ITEMS.filter((i) => i.group === group).map((item) => (
              <button
                key={item.id}
                onClick={() => setActiveSection(item.id)}
                className={clsx(
                  "w-full flex items-center gap-2.5 px-4 py-1.5 text-sm rounded-none transition-colors",
                  activeSection === item.id
                    ? "bg-mantis-600/20 text-mantis-400 font-medium"
                    : "text-gray-400 hover:text-gray-100 hover:bg-surface-hover"
                )}
              >
                <span className="text-base leading-none">{item.icon}</span>
                <span>{item.label}</span>
              </button>
            ))}
          </div>
        ))}
      </div>

      {/* Sources / Settings footer */}
      <div className="border-t border-subtle py-2">
        <button
          onClick={() => setActiveSection("sources")}
          className={clsx(
            "w-full flex items-center gap-2.5 px-4 py-1.5 text-sm transition-colors",
            activeSection === "sources"
              ? "text-mantis-400"
              : "text-gray-500 hover:text-gray-300"
          )}
        >
          <span className="text-base">💾</span>
          <span>ソース管理</span>
        </button>
      </div>
    </nav>
  );
}
