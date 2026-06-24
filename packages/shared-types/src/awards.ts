export type PrestigeTier = "S" | "A" | "B" | "C";

export type AwardBodyType =
  | "industry_award"
  | "film_festival"
  | "national_academy"
  | "regional_award"
  | "critics_award"
  | "other";

export type AwardScope =
  | "global_industry"
  | "major_festival"
  | "national_academy"
  | "indie_discovery"
  | "awards_season"
  | "regional"
  | "critics"
  | "other";

export type AwardCategoryType =
  | "best_picture"
  | "grand_prize"
  | "director"
  | "actor"
  | "actress"
  | "supporting_actor"
  | "supporting_actress"
  | "screenplay"
  | "international_feature"
  | "animation"
  | "documentary"
  | "audience"
  | "technical"
  | "special"
  | "other";

export type AwardTargetType = "work" | "person" | "work_and_person";

export type AwardResultType =
  | "winner"
  | "nominee"
  | "shortlisted"
  | "special_mention"
  | "selection"
  | "unknown";

export type AwardAlertStatus =
  | "up_to_date"
  | "data_stale"
  | "result_season"
  | "nomination_season"
  | "nomination_soon"
  | "scheduled"
  | "unscheduled";

export interface AwardBody {
  id: number;
  name: string;
  displayNameJa: string;
  originalName: string | null;
  sortName: string | null;
  bodyType: AwardBodyType;
  prestigeTier: PrestigeTier;
  awardScope: AwardScope;
  country: string | null;
  city: string | null;
  officialUrl: string | null;
  isActive: boolean;
  displayOrder: number;
  note: string | null;
  categoryCount: number;
  registeredWorkCount: number;
  winnerCount: number;
  currentEditionYear: number;
  currentDataComplete: boolean;
  alertStatus: AwardAlertStatus;
  alertLabel: string;
  dataSourceUrl: string | null;
  wikidataEntityId: string | null;
  scheduleNote: string | null;
}

export interface AwardCategory {
  id: number;
  awardBodyId: number;
  name: string;
  displayNameJa: string;
  originalName: string | null;
  categoryType: AwardCategoryType;
  targetType: AwardTargetType;
  isTopPrize: boolean;
  isMajorCategory: boolean;
  displayOrder: number;
  note: string | null;
}

export interface AwardEdition {
  id: number;
  awardBodyId: number;
  year: number;
  editionNo: number | null;
  startDate: string | null;
  endDate: string | null;
  ceremonyDate: string | null;
  officialUrl: string | null;
  note: string | null;
}

export interface WorkAwardResultView {
  id: number;
  workId: number;
  personId: number | null;
  awardBodyId: number;
  awardEditionId: number | null;
  awardCategoryId: number;
  resultType: AwardResultType;
  sectionName: string | null;
  sourceUrl: string | null;
  confidence: number | null;
  isLocked: boolean;
  note: string | null;
  awardBodyName: string;
  awardBodyDisplayNameJa: string;
  prestigeTier: PrestigeTier;
  awardScope: AwardScope;
  awardYear: number | null;
  categoryName: string;
  categoryDisplayNameJa: string;
  personName: string | null;
}

export interface AwardWork {
  resultId: number;
  workId: number;
  title: string;
  year: number | null;
  posterPath: string | null;
  awardYear: number | null;
  awardCategoryId: number;
  categoryDisplayNameJa: string;
  categoryName: string;
  resultType: AwardResultType;
  personName: string | null;
}
