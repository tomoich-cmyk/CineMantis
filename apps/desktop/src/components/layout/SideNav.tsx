import { useState, useEffect } from "react";
import { getVersion } from "@tauri-apps/api/app";
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
  { id: "favorites",         label: "お気に入り", icon: "★",  group: "スマート" },
  { id: "recently-added",    label: "最近追加",   icon: "🆕", group: "スマート" },
  { id: "recently-played",   label: "最近視聴",   icon: "⏱",  group: "スマート" },
  { id: "high-rated",        label: "高評価",     icon: "⭐", group: "スマート" },
  { id: "unorganized",       label: "未整理",     icon: "!",  group: "スマート" },
  { id: "needs-attention",   label: "要確認",     icon: "⚠",  group: "スマート" },
];

const GROUPS = ["ライブラリ", "スマート"];

export function SideNav() {
  const { activeSection, setActiveSection } = useLibraryStore();
  const [version, setVersion] = useState("");
  useEffect(() => { getVersion().then(setVersion).catch(() => {}); }, []);

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

      {/* Sources / Backup / Audit / Settings footer */}
      <div className="border-t border-subtle py-2">
        {(
          [
            { id: "sources",  icon: "💾",  label: "ソース管理" },
            { id: "backup",   icon: "🛡",  label: "バックアップ" },
            { id: "audit",    icon: "🔬",  label: "監査レポート" },
            { id: "settings", icon: "⚙️", label: "設定" },
            { id: "about",    icon: "🪲",  label: "About" },
          ] as const
        ).map((item) => (
          <button
            key={item.id}
            onClick={() => setActiveSection(item.id)}
            className={clsx(
              "w-full flex items-center gap-2.5 px-4 py-1.5 text-sm transition-colors",
              activeSection === item.id
                ? "text-mantis-400"
                : "text-gray-500 hover:text-gray-300"
            )}
          >
            <span className="text-base">{item.icon}</span>
            <span>{item.label}</span>
          </button>
        ))}
        {/* バージョン表示 */}
        {version && (
          <p className="px-4 pt-1 pb-2 text-[10px] text-gray-700 font-mono select-none">
            v{version}
          </p>
        )}
      </div>
    </nav>
  );
}
