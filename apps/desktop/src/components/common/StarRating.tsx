import { clsx } from "clsx";

interface Props {
  value: number | null;
  max?: number;
  size?: "xs" | "sm" | "md";
  readonly?: boolean;
  onChange?: (v: number) => void;
}

export function StarRating({ value, max = 5, size = "sm", readonly, onChange }: Props) {
  const sizeClass = size === "xs" ? "text-xs" : size === "sm" ? "text-sm" : "text-base";

  return (
    <div className={clsx("flex items-center gap-0.5", sizeClass)}>
      {Array.from({ length: max }, (_, i) => i + 1).map((star) => (
        <button
          key={star}
          type="button"
          disabled={readonly}
          onClick={() => onChange?.(star)}
          className={clsx(
            "leading-none transition-colors",
            readonly ? "cursor-default" : "hover:scale-110",
            (value ?? 0) >= star ? "text-yellow-400" : "text-gray-700"
          )}
        >
          ★
        </button>
      ))}
    </div>
  );
}
