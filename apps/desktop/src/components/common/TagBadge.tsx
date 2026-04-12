import type { Tag } from "@cinemantis/shared-types";

interface Props {
  tag: Tag;
  onRemove?: () => void;
}

export function TagBadge({ tag, onRemove }: Props) {
  return (
    <span
      className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs"
      style={
        tag.color
          ? { backgroundColor: `${tag.color}22`, color: tag.color }
          : { backgroundColor: "#21262d", color: "#8b949e" }
      }
    >
      {tag.name}
      {onRemove && (
        <button
          onClick={onRemove}
          className="leading-none opacity-60 hover:opacity-100 transition-opacity"
        >
          ×
        </button>
      )}
    </span>
  );
}
