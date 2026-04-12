import { create } from "zustand";
import type { NavSection, SortField, SortOrder, WorkType } from "@cinemantis/shared-types";

interface LibraryFilters {
  workType: WorkType | null;
  watchStatus: string | null;
  tagIds: number[];
  sourceId: number | null;
  query: string;
}

interface LibraryStore {
  // Navigation
  activeSection: NavSection;
  setActiveSection: (s: NavSection) => void;

  // Selection
  selectedWorkId: number | null;
  setSelectedWorkId: (id: number | null) => void;

  // Sort
  sortField: SortField;
  sortOrder: SortOrder;
  setSortField: (f: SortField) => void;
  setSortOrder: (o: SortOrder) => void;

  // View
  viewMode: "grid" | "list";
  setViewMode: (m: "grid" | "list") => void;

  // Filters
  filters: LibraryFilters;
  setFilter: <K extends keyof LibraryFilters>(key: K, value: LibraryFilters[K]) => void;
  resetFilters: () => void;
}

const defaultFilters: LibraryFilters = {
  workType: null,
  watchStatus: null,
  tagIds: [],
  sourceId: null,
  query: "",
};

export const useLibraryStore = create<LibraryStore>((set) => ({
  activeSection: "all-movies",
  setActiveSection: (activeSection) => set({ activeSection, selectedWorkId: null }),

  selectedWorkId: null,
  setSelectedWorkId: (selectedWorkId) => set({ selectedWorkId }),

  sortField: "title",
  sortOrder: "asc",
  setSortField: (sortField) => set({ sortField }),
  setSortOrder: (sortOrder) => set({ sortOrder }),

  viewMode: "grid",
  setViewMode: (viewMode) => set({ viewMode }),

  filters: defaultFilters,
  setFilter: (key, value) =>
    set((s) => ({ filters: { ...s.filters, [key]: value } })),
  resetFilters: () => set({ filters: defaultFilters }),
}));
