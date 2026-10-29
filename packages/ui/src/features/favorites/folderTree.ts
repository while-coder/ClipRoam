import type { FavoriteFolder } from "@cliproam/protocol";

export function flattenFavoriteFolders(folders: readonly FavoriteFolder[]) {
  const children = new Map<string | null, FavoriteFolder[]>();
  for (const folder of folders) {
    const siblings = children.get(folder.parentId) ?? [];
    siblings.push(folder);
    children.set(folder.parentId, siblings);
  }
  const rows: Array<FavoriteFolder & { depth: number; path: string; count: number; descendantIds: string[] }> = [];
  function visit(folder: FavoriteFolder, depth: number, path: string): typeof rows[number] {
    const row = { ...folder, depth, path, count: folder.entryCount, descendantIds: [folder.id] };
    rows.push(row);
    for (const child of children.get(folder.id) ?? []) {
      const result = visit(child, depth + 1, `${path} / ${child.name}`);
      row.count += result.count;
      row.descendantIds.push(...result.descendantIds);
    }
    return row;
  }
  for (const root of children.get(null) ?? []) visit(root, 1, root.name);
  return rows;
}
