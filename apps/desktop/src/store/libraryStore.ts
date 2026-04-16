import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { NavSection, SortField, SortOrder, WorkType } from "@cinemantis/shared-types";

export type Density = "compact" | "normal" | "relaxed";

export interface LibraryFilters {
  workType: WorkType | null;
  watchStatus: string | null;
  matchStatus: string | null;
  tagIds: number[];
  sourceId: number | null;
  query: string;
  // 拡張フィルタ (FilterBar)
  yearFrom: number | null;
  yearTo: number | null;
  genre: string | null;
  country: string | null;
  minUserRating: number | null;
  isFavorite: boolean;
}

interface LibraryStore {
  // Navigation
  activeSection: NavSection;
  setActiveSection: (s: NavSection) => void;

  // Selection (DetailPane)
  selectedWorkId: number | null;
  setSelectedWorkId: (id: number | null) => void;

  // Series
  selectedSeriesId: number | null;
  setSelectedSeriesId: (id: number | null) => void;

  // Persons
  selectedPersonId: number | null;
  setSelectedPersonId: (id: number | null) => void;

  // Sort
  sortField: SortField;
  sortOrder: SortOrder;
  setSortField: (f: SortField) => void;
  setSortOrder: (o: SortOrder) => void;

  // View
  viewMode: "grid" | "list";
  setViewMode: (m: "grid" | "list") => void;

  // Density
  density: Density;
  setDensity: (d: Density) => void;

  // Filters
  filters: LibraryFilters;
  setFilter: <K extends keyof LibraryFilters>(key: K, value: LibraryFilters[K]) => void;
  resetFilters: () => void;

  // FilterBar
  filterBarOpen: boolean;
  setFilterBarOpen: (open: boolean) => void;
  toggleFilterBar: () => void;

  // Bulk select mode
  isSelectMode: boolean;
  selectedWorkIds: number[];
  toggleSelectMode: () => void;
  toggleSelectWork: (id: number) => void;
  selectAllWorks: (ids: number[]) => void;
  clearSelection: () => void;
}

const defaultFilters: LibraryFilters = {
  workType: null,
  watchStatus: null,
  matchStatus: null,
  tagIds: [],
  sourceId: null,
  query: "",
  yearFrom: null,
  yearTo: null,
  genre: null,
  country: null,
  minUserRating: null,
  isFavorite: false,
};

export const useLibraryStore = create<LibraryStore>()(
  persist(
    (set) => ({
      // ── Navigation（保存しない） ──────────────────────────────────────────
      activeSection: "all-movies",
      setActiveSection: (activeSection) =>
        set({
          activeSection,
          selectedWorkId: null,
          selectedSeriesId: null,
          selectedPersonId: null,
          isSelectMode: false,
          selectedWorkIds: [],
        }),

      // ── Selection（保存しない） ───────────────────────────────────────────
      selectedWorkId: null,
      setSelectedWorkId: (selectedWorkId) => set({ selectedWorkId }),

      selectedSeriesId: null,
      setSelectedSeriesId: (selectedSeriesId) => set({ selectedSeriesId }),

      selectedPersonId: null,
      setSelectedPersonId: (selectedPersonId) => set({ selectedPersonId }),

      // ── Sort（保存する） ──────────────────────────────────────────────────
      sortField: "title",
      sortOrder: "asc",
      setSortField: (sortField) => set({ sortField }),
      setSortOrder: (sortOrder) => set({ sortOrder }),

      // ── View（保存する） ──────────────────────────────────────────────────
      viewMode: "grid",
      setViewMode: (viewMode) => set({ viewMode }),

      // ── Density（保存する） ───────────────────────────────────────────────
      density: "normal",
      setDensity: (density) => set({ density }),

      // ── Filters（保存する、query は除外） ──────────────────────────────────
      filters: defaultFilters,
      setFilter: (key, value) =>
        set((s) => ({ filters: { ...s.filters, [key]: value } })),
      resetFilters: () => set({ filters: { ...defaultFilters, query: "" } }),

      // ── FilterBar（保存する） ─────────────────────────────────────────────
      filterBarOpen: false,
      setFilterBarOpen: (filterBarOpen) => set({ filterBarOpen }),
      toggleFilterBar: () => set((s) => ({ filterBarOpen: !s.filterBarOpen })),

      // ── Bulk select（保存しない） ─────────────────────────────────────────
      isSelectMode: false,
      selectedWorkIds: [],
      toggleSelectMode: () =>
        set((s) => ({
          isSelectMode: !s.isSelectMode,
          selectedWorkIds: [],
          selectedWorkId: !s.isSelectMode ? null : s.selectedWorkId,
        })),
      toggleSelectWork: (id) =>
        set((s) => ({
          selectedWorkIds: s.selectedWorkIds.includes(id)
            ? s.selectedWorkIds.filter((x) => x !== id)
            : [...s.selectedWorkIds, id],
        })),
      selectAllWorks: (ids) => set({ selectedWorkIds: ids }),
      clearSelection: () => set({ selectedWorkIds: [] }),
    }),
    {
      name: "cinemantis-library-v1",
      // 保存する項目のみ抽出（一時的な選択状態・activeSection は除外）
      partialize: (state) => ({
        sortField:     state.sortField,
        sortOrder:     state.sortOrder,
        viewMode:      state.viewMode,
        density:       state.density,
        filterBarOpen: state.filterBarOpen,
        filters: {
          ...state.filters,
          query: "", // 検索文字列は保存しない
        },
      }),
    }
  )
);
