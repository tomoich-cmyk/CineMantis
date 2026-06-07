import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { NavSection, SortField, SortOrder, WorkType } from "@cinemantis/shared-types";

export type Density = "compact" | "normal" | "relaxed";

export interface LibraryFilters {
  workType: WorkType | null;
  matchStatus: string | null;
  sourceId: number | null;
  query: string;
  // 拡張フィルタ (FilterBar)
  yearFrom: number | null;
  yearTo: number | null;
  genre: string | null;
  country: string | null;
  countryType: string | null;
  mediaCategory: string | null;
  dateAddedFrom: string | null;
  dateAddedTo: string | null;
  personId: number | null;
  seriesId: number | null;
  minUserRating: number | null;
  isFavorite: boolean;
  unorganizedOnly: boolean;
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

  visibleColumns: string[];
  setVisibleColumns: (columns: string[]) => void;
  columnWidths: Record<string, number>;
  setColumnWidth: (key: string, width: number) => void;
  browserPaneHeight: number;
  setBrowserPaneHeight: (height: number) => void;
}

const defaultFilters: LibraryFilters = {
  workType: null,
  matchStatus: null,
  sourceId: null,
  query: "",
  yearFrom: null,
  yearTo: null,
  genre: null,
  country: null,
  countryType: null,
  mediaCategory: null,
  dateAddedFrom: null,
  dateAddedTo: null,
  personId: null,
  seriesId: null,
  minUserRating: null,
  isFavorite: false,
  unorganizedOnly: false,
};

export const DEFAULT_VISIBLE_COLUMNS = [
  "title",
  "releaseYear",
  "mediaCategory",
  "countryType",
  "genreText",
  "myRating",
  "playCount",
  "dateAdded",
  "lastWatchedAt",
  "fileSize",
  "storagePath",
];

export const DEFAULT_COLUMN_WIDTHS: Record<string, number> = {
  title: 260,
  releaseYear: 72,
  mediaCategory: 88,
  countryType: 78,
  genreText: 170,
  myRating: 112,
  playCount: 88,
  watchedStatus: 100,
  dateAdded: 148,
  lastWatchedAt: 148,
  fileSize: 96,
  storagePath: 280,
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

      visibleColumns: DEFAULT_VISIBLE_COLUMNS,
      setVisibleColumns: (visibleColumns) => set({ visibleColumns }),
      columnWidths: DEFAULT_COLUMN_WIDTHS,
      setColumnWidth: (key, width) =>
        set((s) => ({
          columnWidths: {
            ...s.columnWidths,
            [key]: Math.max(56, Math.min(640, Math.round(width))),
          },
        })),
      browserPaneHeight: 176,
      setBrowserPaneHeight: (height) =>
        set({ browserPaneHeight: Math.max(72, Math.min(360, Math.round(height))) }),
    }),
    {
      name: "cinemantis-library-v1",
      merge: (persisted, current) => {
        const state = persisted as Partial<LibraryStore>;
        const visibleColumns = (state.visibleColumns ?? current.visibleColumns)
          .filter((column) => column !== "watchedStatus");
        const withPlayCount = visibleColumns.includes("playCount")
          ? visibleColumns
          : [
              ...visibleColumns.filter((column) => column !== "dateAdded"),
              "playCount",
              ...(visibleColumns.includes("dateAdded") ? ["dateAdded"] : []),
            ];
        const withLastWatched = withPlayCount.includes("lastWatchedAt")
          ? withPlayCount
          : [
              ...withPlayCount.filter((column) => column !== "fileSize" && column !== "storagePath"),
              "lastWatchedAt",
              ...withPlayCount.filter((column) => column === "fileSize" || column === "storagePath"),
            ];
        const nextVisibleColumns = withLastWatched.includes("fileSize")
          ? withLastWatched
          : [
              ...withLastWatched.filter((column) => column !== "storagePath"),
              "fileSize",
              ...(withLastWatched.includes("storagePath") ? ["storagePath"] : []),
            ];
        return {
          ...current,
          ...state,
          filters: state.filters
            ? {
                ...current.filters,
                ...state.filters,
                dateAddedFrom: null,
                dateAddedTo: null,
              }
            : current.filters,
          visibleColumns: nextVisibleColumns,
          columnWidths: { ...DEFAULT_COLUMN_WIDTHS, ...state.columnWidths },
        };
      },
      // 保存する項目のみ抽出（一時的な選択状態・activeSection は除外）
      partialize: (state) => ({
        sortField:     state.sortField,
        sortOrder:     state.sortOrder,
        viewMode:      state.viewMode,
        density:       state.density,
        filterBarOpen: state.filterBarOpen,
        visibleColumns: state.visibleColumns,
        columnWidths: state.columnWidths,
        browserPaneHeight: state.browserPaneHeight,
        filters: {
          ...state.filters,
          query: "", // 検索文字列は保存しない
        },
      }),
    }
  )
);
