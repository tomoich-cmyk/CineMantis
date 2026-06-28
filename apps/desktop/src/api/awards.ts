import { invoke } from "@tauri-apps/api/core";
import type {
  AwardBody,
  AwardCategory,
  AwardEdition,
  AwardResultType,
  AwardWork,
  WorkAwardResultView,
} from "@cinemantis/shared-types";

interface AwardBodyRow {
  id: number;
  name: string;
  display_name_ja: string;
  original_name: string | null;
  sort_name: string | null;
  body_type: AwardBody["bodyType"];
  prestige_tier: AwardBody["prestigeTier"];
  award_scope: AwardBody["awardScope"];
  country: string | null;
  city: string | null;
  official_url: string | null;
  is_active: boolean;
  display_order: number;
  note: string | null;
  category_count: number;
  registered_work_count: number;
  winner_count: number;
  current_edition_year: number;
  current_data_complete: boolean;
  alert_status: AwardBody["alertStatus"];
  alert_label: string;
  data_source_url: string | null;
  wikidata_entity_id: string | null;
  schedule_note: string | null;
}

interface AwardCategoryRow {
  id: number;
  award_body_id: number;
  name: string;
  display_name_ja: string;
  original_name: string | null;
  category_type: AwardCategory["categoryType"];
  target_type: AwardCategory["targetType"];
  is_top_prize: boolean;
  is_major_category: boolean;
  display_order: number;
  note: string | null;
}

interface WorkAwardResultViewRow {
  id: number;
  work_id: number;
  person_id: number | null;
  award_body_id: number;
  award_edition_id: number | null;
  award_category_id: number;
  result_type: AwardResultType;
  section_name: string | null;
  source_url: string | null;
  confidence: number | null;
  is_locked: boolean;
  note: string | null;
  award_body_name: string;
  award_body_display_name_ja: string;
  prestige_tier: WorkAwardResultView["prestigeTier"];
  award_scope: WorkAwardResultView["awardScope"];
  award_year: number | null;
  category_name: string;
  category_display_name_ja: string;
  person_name: string | null;
}

interface AwardEditionRow {
  id: number;
  award_body_id: number;
  year: number;
  edition_no: number | null;
  start_date: string | null;
  end_date: string | null;
  ceremony_date: string | null;
  official_url: string | null;
  note: string | null;
}

interface AwardWorkRow {
  result_id: number;
  work_id: number;
  title: string;
  year: number | null;
  poster_path: string | null;
  award_year: number | null;
  award_category_id: number;
  category_display_name_ja: string;
  category_name: string;
  result_type: AwardResultType;
  person_name: string | null;
}

export interface AddWorkAwardResultInput {
  workId: number;
  personId?: number | null;
  awardBodyId: number;
  awardEditionId?: number | null;
  awardYear?: number | null;
  awardCategoryId: number;
  resultType: AwardResultType;
  sectionName?: string | null;
  sourceUrl?: string | null;
  note?: string | null;
  isLocked?: boolean | null;
}

export interface UpdateWorkAwardResultInput {
  id: number;
  resultType?: AwardResultType | null;
  sectionName?: string | null;
  sourceUrl?: string | null;
  note?: string | null;
  isLocked?: boolean | null;
}

export interface AwardWorkFilter {
  awardBodyId?: number | null;
  awardCategoryId?: number | null;
  prestigeTier?: string | null;
  resultType?: AwardResultType | null;
}

export interface AwardImportJobSummary {
  jobId: number;
  status: "done" | "error";
  totalItems: number;
  insertedItems: number;
  skippedItems: number;
  errorMessage?: string | null;
}

export interface AwardImportMatchSummary {
  jobId: number;
  totalItems: number;
  highConfidence: number;
  needsReview: number;
  lowConfidence: number;
  unmatched: number;
  alreadyMatched: number;
}

export interface MatchCandidateView {
  id: number;
  workId: number;
  workTitle: string;
  workOriginalTitle: string | null;
  workYear: number | null;
  workSourcePath: string | null;
  workTmdbId: string | null;
  workImdbId: string | null;
  score: number;
  matchMethod: string;
  isSelected: boolean;
}

export interface AwardImportItemView {
  id: number;
  jobId: number;
  awardBodyId: number;
  awardBodyName: string;
  awardCategoryId: number | null;
  awardCategoryName: string | null;
  rawYear: number | null;
  rawResultType: string;
  rawFilmId: string;
  rawTitleJa: string | null;
  rawTitleEn: string | null;
  rawImdbId: string | null;
  rawTmdbId: string | null;
  rawSourceUrl: string | null;
  rawAwardNameEn: string | null;
  rawAwardNameJa: string | null;
  matchedWorkId: number | null;
  matchedWorkTitle: string | null;
  matchedWorkYear: number | null;
  matchedWorkTmdbId: string | null;
  matchScore: number | null;
  matchMethod: string | null;
  status: string;
  approvedAt: string | null;
  alreadyConfirmed: boolean;
  candidates: MatchCandidateView[];
}

export interface WorkSearchResult {
  id: number;
  title: string;
  originalTitle: string | null;
  year: number | null;
  tmdbId: string | null;
  imdbId: string | null;
  sourcePath: string | null;
}

export interface BulkOperationResult {
  approved: number;
  rejected: number;
  skipped: number;
  errors: string[];
}

export async function listAwardBodies(): Promise<AwardBody[]> {
  const rows = await invoke<AwardBodyRow[]>("list_award_bodies");
  return rows.map(toAwardBody);
}

export async function getAwardBodyDetail(awardBodyId: number): Promise<AwardBody | null> {
  const row = await invoke<AwardBodyRow | null>("get_award_body_detail", { awardBodyId });
  return row ? toAwardBody(row) : null;
}

export async function listAwardCategories(awardBodyId: number): Promise<AwardCategory[]> {
  const rows = await invoke<AwardCategoryRow[]>("list_award_categories", { awardBodyId });
  return rows.map(toAwardCategory);
}

export async function getOrCreateAwardEdition(
  awardBodyId: number,
  year: number,
): Promise<AwardEdition> {
  const row = await invoke<AwardEditionRow>("get_or_create_award_edition", {
    input: { awardBodyId, year },
  });
  return {
    id: row.id,
    awardBodyId: row.award_body_id,
    year: row.year,
    editionNo: row.edition_no,
    startDate: row.start_date,
    endDate: row.end_date,
    ceremonyDate: row.ceremony_date,
    officialUrl: row.official_url,
    note: row.note,
  };
}

export async function listWorkAwards(workId: number): Promise<WorkAwardResultView[]> {
  const rows = await invoke<WorkAwardResultViewRow[]>("list_work_awards", { workId });
  return rows.map(toWorkAward);
}

export async function addWorkAwardResult(
  input: AddWorkAwardResultInput,
): Promise<WorkAwardResultView> {
  const row = await invoke<WorkAwardResultViewRow>("add_work_award_result", { input });
  return toWorkAward(row);
}

export async function updateWorkAwardResult(
  input: UpdateWorkAwardResultInput,
): Promise<WorkAwardResultView> {
  const row = await invoke<WorkAwardResultViewRow>("update_work_award_result", { input });
  return toWorkAward(row);
}

export async function deleteWorkAwardResult(id: number): Promise<void> {
  return invoke("delete_work_award_result", { id });
}

export async function listAwardWinningWorks(filter: AwardWorkFilter): Promise<AwardWork[]> {
  const rows = await invoke<AwardWorkRow[]>("list_award_winning_works", { filter });
  return rows.map((r) => ({
    resultId: r.result_id,
    workId: r.work_id,
    title: r.title,
    year: r.year,
    posterPath: r.poster_path,
    awardYear: r.award_year,
    awardCategoryId: r.award_category_id,
    categoryDisplayNameJa: r.category_display_name_ja,
    categoryName: r.category_name,
    resultType: r.result_type,
    personName: r.person_name,
  }));
}

export async function fetchWikidataAwardItems(
  awardBodyId: number,
  awardCategoryId?: number | null,
  year?: number | null,
): Promise<AwardImportJobSummary> {
  return invoke<AwardImportJobSummary>("fetch_wikidata_award_items", {
    awardBodyId,
    awardCategoryId,
    year,
  });
}

export async function matchAwardImportItems(jobId: number): Promise<AwardImportMatchSummary> {
  return invoke<AwardImportMatchSummary>("match_award_import_items", { jobId });
}

export async function listAwardImportItems(options: {
  jobId?: number | null;
  status?: string | null;
  onlyUnmatched?: boolean | null;
} = {}): Promise<AwardImportItemView[]> {
  return invoke<AwardImportItemView[]>("list_award_import_items", options);
}

export async function selectAwardMatchCandidate(
  importItemId: number,
  candidateId: number,
): Promise<void> {
  return invoke("select_award_match_candidate", { importItemId, candidateId });
}

export async function approveAwardImportItem(
  importItemId: number,
): Promise<WorkAwardResultView> {
  const row = await invoke<WorkAwardResultViewRow>("approve_award_import_item", {
    importItemId,
  });
  return toWorkAward(row);
}

export async function rejectAwardImportItem(importItemId: number): Promise<void> {
  return invoke("reject_award_import_item", { importItemId });
}

export async function searchWorksForAwardMatch(
  query: string,
  year?: number | null,
): Promise<WorkSearchResult[]> {
  return invoke<WorkSearchResult[]>("search_works_for_award_match", { query, year });
}

export async function addManualAwardMatchCandidate(
  importItemId: number,
  workId: number,
): Promise<number> {
  return invoke<number>("add_manual_award_match_candidate", { importItemId, workId });
}

export async function bulkApproveAwardImportItems(
  importItemIds: number[],
): Promise<BulkOperationResult> {
  return invoke<BulkOperationResult>("bulk_approve_award_import_items", { importItemIds });
}

export async function bulkRejectAwardImportItems(
  importItemIds: number[],
): Promise<BulkOperationResult> {
  return invoke<BulkOperationResult>("bulk_reject_award_import_items", { importItemIds });
}

function toAwardBody(r: AwardBodyRow): AwardBody {
  return {
    id: r.id,
    name: r.name,
    displayNameJa: r.display_name_ja,
    originalName: r.original_name,
    sortName: r.sort_name,
    bodyType: r.body_type,
    prestigeTier: r.prestige_tier,
    awardScope: r.award_scope,
    country: r.country,
    city: r.city,
    officialUrl: r.official_url,
    isActive: r.is_active,
    displayOrder: r.display_order,
    note: r.note,
    categoryCount: r.category_count,
    registeredWorkCount: r.registered_work_count,
    winnerCount: r.winner_count,
    currentEditionYear: r.current_edition_year,
    currentDataComplete: r.current_data_complete,
    alertStatus: r.alert_status,
    alertLabel: r.alert_label,
    dataSourceUrl: r.data_source_url,
    wikidataEntityId: r.wikidata_entity_id,
    scheduleNote: r.schedule_note,
  };
}

function toAwardCategory(r: AwardCategoryRow): AwardCategory {
  return {
    id: r.id,
    awardBodyId: r.award_body_id,
    name: r.name,
    displayNameJa: r.display_name_ja,
    originalName: r.original_name,
    categoryType: r.category_type,
    targetType: r.target_type,
    isTopPrize: r.is_top_prize,
    isMajorCategory: r.is_major_category,
    displayOrder: r.display_order,
    note: r.note,
  };
}

function toWorkAward(r: WorkAwardResultViewRow): WorkAwardResultView {
  return {
    id: r.id,
    workId: r.work_id,
    personId: r.person_id,
    awardBodyId: r.award_body_id,
    awardEditionId: r.award_edition_id,
    awardCategoryId: r.award_category_id,
    resultType: r.result_type,
    sectionName: r.section_name,
    sourceUrl: r.source_url,
    confidence: r.confidence,
    isLocked: r.is_locked,
    note: r.note,
    awardBodyName: r.award_body_name,
    awardBodyDisplayNameJa: r.award_body_display_name_ja,
    prestigeTier: r.prestige_tier,
    awardScope: r.award_scope,
    awardYear: r.award_year,
    categoryName: r.category_name,
    categoryDisplayNameJa: r.category_display_name_ja,
    personName: r.person_name,
  };
}
