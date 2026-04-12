import { invoke } from "@tauri-apps/api/core";
import type { Tag } from "@cinemantis/shared-types";

interface TagRow {
  id: number;
  name: string;
  color: string | null;
  tag_type: string;
}

function toTag(r: TagRow): Tag {
  return {
    id: r.id,
    name: r.name,
    color: r.color,
    tagType: r.tag_type as Tag["tagType"],
  };
}

export async function listTags(): Promise<Tag[]> {
  const rows = await invoke<TagRow[]>("list_tags");
  return rows.map(toTag);
}

export async function addTag(name: string, color?: string): Promise<number> {
  return invoke<number>("add_tag", { payload: { name, color: color ?? null } });
}

export async function tagWork(workId: number, tagId: number): Promise<void> {
  return invoke("tag_work", { work_id: workId, tag_id: tagId });
}

export async function untagWork(workId: number, tagId: number): Promise<void> {
  return invoke("untag_work", { work_id: workId, tag_id: tagId });
}

export async function listWorkTags(workId: number): Promise<Tag[]> {
  const rows = await invoke<TagRow[]>("list_work_tags", { work_id: workId });
  return rows.map(toTag);
}
