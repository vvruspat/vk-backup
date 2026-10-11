import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  Collection,
  Kind,
  Page,
  Progress,
  Selection,
  Source,
  User,
} from "./types";

export const native = isTauri();
export const demo =
  !native && new URLSearchParams(location.search).get("demo") === "1";
const delay = (ms = 500) => new Promise((resolve) => setTimeout(resolve, ms));
const demoUser: User = {
  id: 10001,
  name: "Демонстрационный аккаунт",
  avatar: null,
};
const demoSources: Source[] = [
  { ...demoUser, community: false },
  { id: -20001, name: "Путешествия рядом", avatar: null, community: true },
  { id: -20002, name: "Дизайн-сообщество", avatar: null, community: true },
];
const scenes = ["coast", "mountain", "city", "forest", "sunset", "snow"];
export function demoCatalog(ownerId: number): Collection[] {
  const names =
    ownerId < 0
      ? ["Встречи сообщества", "Наша команда", "Фото участников"]
      : [
          "Лето у моря",
          "Поход в горы",
          "Городские истории",
          "Моменты с друзьями",
          "На закате",
          "Зимние каникулы",
        ];
  return [
    ...names.map((title, i) => ({
      id: `${ownerId}:photos:${i}`,
      ownerId,
      kind: "photos" as const,
      title,
      count: [48, 36, 64, 28, 19, 42][i],
      cover: `/demo/${scenes[i]}.svg`,
      description: "Демонстрационный альбом",
      available: true,
      error: null,
    })),
    {
      id: `${ownerId}:videos:all`,
      ownerId,
      kind: "videos",
      title: "Все видеозаписи",
      count: 12,
      cover: "/demo/coast.svg",
      description: "Лучшее доступное качество",
      available: true,
      error: null,
    },
    {
      id: `${ownerId}:documents`,
      ownerId,
      kind: "documents",
      title: "Документы",
      count: 8,
      cover: null,
      description: "Оригиналы файлов",
      available: true,
      error: null,
    },
    ...(
      [
        "profile",
        "wall",
        ...(ownerId > 0 ? ["friends", "messages"] : []),
      ] as Kind[]
    ).map((kind) => ({
      id: `${ownerId}:${kind}`,
      ownerId,
      kind,
      title: (
        {
          profile: ownerId < 0 ? "Информация о сообществе" : "Профиль",
          wall: "Записи на стене",
          friends: "Друзья",
          messages: "Переписка",
        } as Record<string, string>
      )[kind],
      count: kind === "profile" ? 1 : 128,
      cover: null,
      description: "Экспорт в JSON",
      available: kind !== "messages",
      error:
        kind === "messages"
          ? "Демонстрация ограничений: ВК не предоставил доступ к переписке."
          : null,
    })),
  ];
}

let simulated: Progress | null = null;
let timer: ReturnType<typeof setInterval> | null = null;
let listeners: ((p: Progress) => void)[] = [];

export const backend = {
  connect: async (token: string): Promise<User> => {
    if (demo) {
      await delay();
      return demoUser;
    }
    return invoke("connect_token", { token });
  },
  sources: async (): Promise<{ items: Source[]; warning: string | null }> => {
    if (demo) return { items: demoSources, warning: null };
    return invoke("sources");
  },
  catalog: async (ownerId: number): Promise<Collection[]> => {
    if (demo) {
      await delay();
      return demoCatalog(ownerId);
    }
    return invoke("catalog", { ownerId });
  },
  items: async (collection: Collection, offset: number): Promise<Page> => {
    if (demo) {
      await delay(200);
      const n = Math.min(20, collection.count - offset);
      return {
        total: collection.count,
        nextOffset: offset + n < collection.count ? offset + n : null,
        items: Array.from({ length: n }, (_, i) => ({
          id: `${collection.ownerId}_${offset + i}`,
          title: `${collection.kind === "videos" ? "Видео" : "Фотография"} ${offset + i + 1}`,
          thumbnail: `/demo/${scenes[(offset + i) % scenes.length]}.svg`,
          duration: collection.kind === "videos" ? 125 : null,
          date: 1750000000,
          downloadable: true,
          reason: null,
        })),
      };
    }
    return invoke("collection_items", { collectionId: collection.id, offset });
  },
  folder: async (): Promise<string | null> => {
    if (demo) return "/Демонстрация/VK Backup";
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Папка для бэкапа ВК",
    });
    return typeof selected === "string" ? selected : null;
  },
  login: async (clientId: string, mode: string, scopes: string) =>
    invoke<void>("begin_login", { clientId, mode, scopes }),
  miniappLogin: async (appId: string, scopes: string) =>
    invoke<{ pairCode: string; expiresAt: number }>("begin_miniapp_login", {
      appId,
      scopes,
    }),
  cancelLogin: async () => invoke<void>("cancel_login"),
  logout: async () => {
    if (!demo) await invoke("logout");
  },
  tools: async (): Promise<boolean> => (demo ? false : invoke("tools_status")),
  start: async (
    folder: string,
    selections: Selection[],
    useYtdlp: boolean,
  ): Promise<Progress> => {
    if (!demo) return invoke("start_backup", { folder, selections, useYtdlp });
    const all = demoSources.flatMap((s) => demoCatalog(s.id));
    const total = selections.reduce(
      (n, s) =>
        n +
        (s.itemIds?.length ??
          Math.max(
            0,
            (all.find((c) => c.id === s.collectionId)?.count ?? 1) -
              s.excludedIds.length,
          )),
      0,
    );
    simulated = {
      id: crypto.randomUUID(),
      status: "running",
      phase: "scanning",
      total,
      completed: 0,
      skipped: 0,
      failed: 0,
      unavailable: 0,
      bytes: 0,
      current: "Демонстрация: собираем список…",
      currentBytes: 0,
      currentTotal: null,
      speed: 0,
      folder,
      errors: [],
      startedAt: Date.now() / 1000,
    };
    if (timer) clearInterval(timer);
    timer = setInterval(() => {
      if (!simulated || simulated.status !== "running") return;
      simulated.phase = "downloading";
      simulated.completed = Math.min(total, simulated.completed + 2);
      simulated.bytes = simulated.completed * 2048000;
      simulated.current = `Демонстрация: файл ${Math.min(total, simulated.completed + 1)}.jpg`;
      simulated.speed = 5242880;
      simulated.currentTotal = 2048000;
      simulated.currentBytes = 1258000;
      if (simulated.completed === total) {
        simulated.status = "completed";
        simulated.phase = "finished";
        simulated.speed = 0;
        if (timer) clearInterval(timer);
      }
      listeners.forEach((fn) => fn({ ...simulated! }));
    }, 350);
    return { ...simulated };
  },
  control: async (id: string, action: string) => {
    if (!demo) return invoke<void>("control_backup", { id, action });
    if (simulated) {
      simulated.status =
        action === "pause"
          ? "paused"
          : action === "resume"
            ? "running"
            : "canceled";
      if (action === "cancel") {
        simulated.phase = "finished";
        if (timer) clearInterval(timer);
      }
      listeners.forEach((fn) => fn({ ...simulated! }));
    }
  },
  status: async (): Promise<Progress | null> =>
    demo ? simulated : invoke("backup_status"),
  openFolder: async () => {
    if (!demo) await invoke("open_backup_folder");
  },
  progress: async (fn: (p: Progress) => void) => {
    if (demo) {
      listeners.push(fn);
      return () => {
        listeners = listeners.filter((f) => f !== fn);
      };
    }
    return listen<Progress>("backup-progress", (e) => fn(e.payload));
  },
};
