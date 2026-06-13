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
  const rating = value ?? 0;

  return (
    <div className={clsx("flex items-center gap-0.5", sizeClass)}>
      {Array.from({ length: max }, (_, i) => i + 1).map((star) => {
        const fill = Math.max(0, Math.min(1, rating - (star - 1))) * 100;
        return (
          <span key={star} className="relative inline-block leading-none">
            <span className="text-gray-700">★</span>
            <span className="absolute inset-0 overflow-hidden text-yellow-400" style={{ width: `${fill}%` }}>★</span>
            {!readonly && (
              <span className="absolute inset-0 flex">
                <button
                  type="button"
                  aria-label={`${star - 0.5} 点`}
                  onClick={(event) => {
                    event.stopPropagation();
                    const next = star - 0.5;
                    onChange?.(rating === next ? 0 : next);
                  }}
                  className="h-full w-1/2"
                />
                <button
                  type="button"
                  aria-label={`${star} 点`}
                  onClick={(event) => {
                    event.stopPropagation();
                    onChange?.(rating === star ? 0 : star);
                  }}
                  className="h-full w-1/2"
                />
              </span>
            )}
          </span>
        );
      })}
    </div>
  );
}
