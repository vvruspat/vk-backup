export type Kind =
  | "photos"
  | "videos"
  | "documents"
  | "profile"
  | "wall"
  | "friends"
  | "messages";
export type User = { id: number; name: string; avatar: string | null };
export type Source = {
  id: number;
  name: string;
  avatar: string | null;
  community: boolean;
};
export type Collection = {
  id: string;
  ownerId: number;
  kind: Kind;
  title: string;
  count: number;
  cover: string | null;
  description: string;
  available: boolean;
  error: string | null;
};
export type Media = {
  id: string;
  title: string;
  thumbnail: string | null;
  duration: number | null;
  date: number | null;
  downloadable: boolean;
  reason: string | null;
};
export type Page = { items: Media[]; total: number; nextOffset: number | null };
export type Selection = {
  collectionId: string;
  itemIds: string[] | null;
  excludedIds: string[];
};
export type Progress = {
  id: string;
  status:
    | "running"
    | "paused"
    | "canceling"
    | "completed"
    | "partial"
    | "failed"
    | "canceled";
  phase: "scanning" | "downloading" | "finished";
  total: number;
  completed: number;
  skipped: number;
  failed: number;
  unavailable: number;
  bytes: number;
  current: string;
  currentBytes: number;
  currentTotal: number | null;
  speed: number;
  folder: string;
  errors: string[];
  startedAt: number;
};

export const isMedia = (kind: Kind) =>
  ["photos", "videos", "documents"].includes(kind);
export const isBusy = (p: Progress | null) =>
  !!p && ["running", "paused", "canceling"].includes(p.status);
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 Б";
  const index = Math.min(3, Math.floor(Math.log(bytes) / Math.log(1024)));
  return `${(bytes / 1024 ** index).toLocaleString("ru-RU", { maximumFractionDigits: index ? 1 : 0 })} ${["Б", "КБ", "МБ", "ГБ"][index]}`;
}
export function formatCount(n: number): string {
  return n.toLocaleString("ru-RU");
}
export function processed(p: Progress) {
  return p.completed + p.skipped + p.failed + p.unavailable;
}
export function percent(p: Progress) {
  return p.total ? Math.min(100, (processed(p) / p.total) * 100) : 0;
}
