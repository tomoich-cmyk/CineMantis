import { useState } from "react";
import { clsx } from "clsx";
import { usePersonsList } from "@/hooks/usePersons";
import { useLibraryStore } from "@/store/libraryStore";
import type { PersonSummary } from "@/api/persons";

// ─── ロール定義 ───────────────────────────────────────────────────────────────

const ROLE_TABS = [
  { value: null,       label: "すべて" },
  { value: "director", label: "監督" },
  { value: "writer",   label: "脚本" },
  { value: "cast",     label: "出演" },
] as const;

const ROLE_LABELS: Record<string, string> = {
  director: "監督",
  writer:   "脚本",
  cast:     "出演",
};

// ─── 人物カード ───────────────────────────────────────────────────────────────

function PersonCard({
  person,
  onClick,
}: {
  person: PersonSummary;
  onClick: () => void;
}) {
  const roleList = person.roles
    .split(",")
    .filter(Boolean)
    .map((r) => ROLE_LABELS[r] ?? r);

  return (
    <div
      onClick={onClick}
      className="group flex items-center gap-3 px-4 py-2.5 cursor-pointer hover:bg-surface-hover transition-colors border-b border-surface-border"
    >
      {/* Avatar placeholder */}
      <div className="w-9 h-9 flex-shrink-0 rounded-full bg-surface flex items-center justify-center text-gray-600 text-sm font-medium overflow-hidden">
        {person.name.slice(0, 1)}
      </div>

      {/* Info */}
      <div className="flex-1 min-w-0">
        <p className="text-sm text-gray-200 font-medium truncate">{person.name}</p>
        <p className="text-xs text-gray-600 truncate">{roleList.join(" / ")}</p>
      </div>

      {/* Work count */}
      <span className="flex-shrink-0 text-xs text-gray-600 font-mono bg-surface px-1.5 py-0.5 rounded">
        {person.work_count}
      </span>
    </div>
  );
}

// ─── メイン画面 ──────────────────────────────────────────────────────────────

export function PersonsScreen() {
  const { setSelectedPersonId } = useLibraryStore();
  const [roleFilter, setRoleFilter] = useState<string | null>(null);
  const [query, setQuery] = useState("");

  const { data: persons = [], isLoading } = usePersonsList(roleFilter);

  const filtered = query
    ? persons.filter((p) =>
        p.name.toLowerCase().includes(query.toLowerCase())
      )
    : persons;

  return (
    <div className="flex-1 flex flex-col min-h-0 overflow-hidden">
      {/* Header */}
      <div className="flex flex-col gap-2 px-4 py-3 border-b border-subtle bg-surface-elevated flex-shrink-0">
        <div className="flex items-center justify-between">
          <h1 className="text-sm font-semibold text-gray-200">人物</h1>
          <span className="text-xs text-gray-600">{filtered.length} 件</span>
        </div>

        <div className="flex items-center gap-2">
          {/* 検索 */}
          <input
            type="search"
            placeholder="名前で検索…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="flex-1 bg-surface border border-subtle rounded px-3 py-1 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-mantis-600 transition-colors"
          />

          {/* ロールフィルタ */}
          <div className="flex gap-1">
            {ROLE_TABS.map((tab) => (
              <button
                key={String(tab.value)}
                onClick={() => setRoleFilter(tab.value)}
                className={clsx(
                  "px-2.5 py-1 text-xs rounded transition-colors",
                  roleFilter === tab.value
                    ? "bg-mantis-700/40 text-mantis-300"
                    : "text-gray-500 hover:text-gray-300"
                )}
              >
                {tab.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* List */}
      <div className="flex-1 overflow-y-auto">
        {isLoading ? (
          <div className="flex items-center justify-center h-40 text-gray-600 animate-pulse text-sm">
            読み込み中…
          </div>
        ) : filtered.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-40 gap-3 text-gray-600">
            <span className="text-4xl opacity-20">👤</span>
            <p className="text-sm">
              {persons.length === 0
                ? "人物データがありません"
                : "該当する人物がいません"}
            </p>
            {persons.length === 0 && (
              <p className="text-xs text-gray-700 text-center max-w-xs">
                TMDb と照合すると、監督・脚本・出演者が自動で登録されます
              </p>
            )}
          </div>
        ) : (
          <div>
            {filtered.map((person) => (
              <PersonCard
                key={person.id}
                person={person}
                onClick={() => setSelectedPersonId(person.id)}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
