import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { clsx } from "clsx";
import { useLibraryStore } from "@/store/libraryStore";
import type { NavSection } from "@cinemantis/shared-types";

interface NavItem {
  id: NavSection;
  label: string;
  icon: string;
  group: "library" | "smart";
}

const NAV_ITEMS: NavItem[] = [
  { id: "all-movies", label: "映画", icon: "🎬", group: "library" },
  { id: "series", label: "シリーズ", icon: "🧩", group: "library" },
  { id: "persons", label: "人物", icon: "👤", group: "library" },
  { id: "awards", label: "映画賞・映画祭", icon: "🏆", group: "library" },
  { id: "unorganized", label: "未整理", icon: "!", group: "smart" },
  { id: "needs-attention", label: "要確認", icon: "△", group: "smart" },
];

const GROUP_LABELS = {
  library: "ライブラリ",
  smart: "スマート",
} as const;

export function SideNav() {
  const { activeSection, setActiveSection } = useLibraryStore();
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  return (
    <nav className="w-48 flex-shrink-0 flex flex-col bg-surface-elevated border-r border-subtle overflow-y-auto">
      <div className="flex items-center gap-2 px-4 py-4 border-b border-subtle">
        <span className="text-mantis-500 font-bold text-base tracking-tight">CineMantis</span>
      </div>

      <div className="flex-1 py-2">
        {(["library", "smart"] as const).map((group) => (
          <div key={group} className="mb-1">
            <div className="px-4 pt-3 pb-1 text-xs font-semibold text-gray-500 uppercase tracking-wider">
              {GROUP_LABELS[group]}
            </div>
            {NAV_ITEMS.filter((item) => item.group === group).map((item) => (
              <button
                key={item.id}
                onClick={() => setActiveSection(item.id)}
                className={clsx(
                  "w-full flex items-center gap-2.5 px-4 py-1.5 text-sm rounded-none transition-colors",
                  activeSection === item.id
                    ? "bg-mantis-600/20 text-mantis-400 font-medium"
                    : "text-gray-400 hover:text-gray-100 hover:bg-surface-hover",
                )}
              >
                <span className="text-base leading-none">{item.icon}</span>
                <span>{item.label}</span>
              </button>
            ))}
          </div>
        ))}
      </div>

      <div className="border-t border-subtle py-2">
        {(
          [
            { id: "sources", icon: "💾", label: "ソース管理" },
            { id: "backup", icon: "▣", label: "バックアップ" },
            { id: "audit", icon: "🔬", label: "監査レポート" },
            { id: "settings", icon: "⚙", label: "設定" },
            { id: "about", icon: "▥", label: "About" },
          ] as const
        ).map((item) => (
          <button
            key={item.id}
            onClick={() => setActiveSection(item.id)}
            className={clsx(
              "w-full flex items-center gap-2.5 px-4 py-1.5 text-sm transition-colors",
              activeSection === item.id
                ? "text-mantis-400"
                : "text-gray-500 hover:text-gray-300",
            )}
          >
            <span className="text-base">{item.icon}</span>
            <span>{item.label}</span>
          </button>
        ))}
        {version && (
          <p className="px-4 pt-1 pb-2 text-[10px] text-gray-700 font-mono select-none">
            v{version}
          </p>
        )}
      </div>
    </nav>
  );
}
