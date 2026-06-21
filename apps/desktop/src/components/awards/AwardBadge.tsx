import { clsx } from "clsx";
import type { AwardResultType, PrestigeTier } from "@cinemantis/shared-types";

export const RESULT_LABELS: Record<AwardResultType, string> = {
  winner: "受賞",
  nominee: "ノミネート",
  shortlisted: "候補",
  special_mention: "特別表彰",
  selection: "選出",
  unknown: "不明",
};

export function AwardTierBadge({ tier }: { tier: PrestigeTier }) {
  return (
    <span
      className={clsx(
        "inline-flex h-5 min-w-5 items-center justify-center rounded px-1.5 text-[11px] font-semibold",
        tier === "S" && "bg-yellow-500/15 text-yellow-300",
        tier === "A" && "bg-blue-500/15 text-blue-300",
        tier === "B" && "bg-mantis-500/15 text-mantis-300",
        tier === "C" && "bg-gray-500/15 text-gray-300",
      )}
    >
      {tier}
    </span>
  );
}

export function ResultBadge({ result }: { result: AwardResultType }) {
  return (
    <span
      className={clsx(
        "inline-flex items-center rounded px-1.5 py-0.5 text-[11px]",
        result === "winner" && "bg-yellow-500/15 text-yellow-300",
        result === "nominee" && "bg-blue-500/15 text-blue-300",
        result === "selection" && "bg-mantis-500/15 text-mantis-300",
        !["winner", "nominee", "selection"].includes(result) && "bg-gray-600/20 text-gray-400",
      )}
    >
      {RESULT_LABELS[result]}
    </span>
  );
}
