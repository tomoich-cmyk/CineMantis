import { clsx } from "clsx";

interface Props {
  value: number | null;
  max?: number;
  size?: "xs" | "sm" | "md";
  readonly?: boolean;
  onChange?: (v: number) => void;
}

function clampRating(value: number, max: number) {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(max, Math.round(value * 10) / 10));
}

function formatRating(value: number | null) {
  return value === null ? "未評価" : value.toFixed(1);
}

export function StarRating({ value, max = 10, size = "sm", readonly, onChange }: Props) {
  const sizeClass = size === "xs" ? "text-[11px]" : size === "sm" ? "text-xs" : "text-sm";
  const rating = value === null ? null : clampRating(value, max);
  const percent = rating === null ? 0 : (rating / max) * 100;

  if (readonly) {
    return (
      <span className={clsx("inline-flex items-center gap-1 font-mono tabular-nums", sizeClass)}>
        {rating === null ? (
          <span className="text-gray-700">未評価</span>
        ) : (
          <>
            <span className="text-yellow-400 leading-none">★</span>
            <span className="text-yellow-300">{formatRating(rating)}</span>
          </>
        )}
      </span>
    );
  }

  return (
    <div className={clsx("flex items-center gap-2", sizeClass)} onClick={(event) => event.stopPropagation()}>
      <span className="text-yellow-400 leading-none">★</span>
      <input
        type="number"
        min={0}
        max={max}
        step={0.1}
        value={rating ?? ""}
        placeholder="未評価"
        onChange={(event) => {
          const raw = event.target.value;
          onChange?.(raw === "" ? 0 : clampRating(Number(raw), max));
        }}
        className="h-6 w-14 rounded border border-subtle bg-surface px-1.5 text-right font-mono text-yellow-300 outline-none focus:border-mantis-600"
      />
      <div className="h-1.5 w-16 overflow-hidden rounded-full bg-gray-800">
        <div className="h-full bg-yellow-400" style={{ width: `${percent}%` }} />
      </div>
      <span className="font-mono text-gray-600">/ {max}</span>
    </div>
  );
}
