import { useEffect, useMemo, useRef, useState } from "react";
import * as Tabs from "@radix-ui/react-tabs";
import { listen } from "@tauri-apps/api/event";
import {
  Archive,
  ArrowDownToLine,
  ArrowRight,
  Check,
  CheckCheck,
  ChevronRight,
  CircleCheck,
  CircleHelp,
  Clock3,
  Download,
  ExternalLink,
  FileJson,
  Folder,
  FolderOpen,
  HardDrive,
  Image,
  Info,
  LayoutGrid,
  LoaderCircle,
  LockKeyhole,
  LogOut,
  Pause,
  Play,
  RefreshCw,
  Search,
  Settings2,
  ShieldCheck,
  Sparkles,
  UsersRound,
  X,
  AlertCircle,
} from "lucide-react";
import { backend, demo, native } from "./bridge";
import { Bar, CheckBox, icons, Modal } from "./components";
import {
  formatBytes,
  formatCount,
  isBusy,
  isMedia,
  percent,
  processed,
  type Collection,
  type Media,
  type Progress,
  type Selection,
  type Source,
  type User,
} from "./types";
import "./styles.css";

type View = "photos" | "videos" | "documents" | "data";
const views: { id: View; title: string; description: string }[] = [
  {
    id: "photos",
    title: "Фотоальбомы",
    description: "Ваши фотографии, в привычном порядке.",
  },
  {
    id: "videos",
    title: "Видеозаписи",
    description: "Сохраните видео в лучшем доступном качестве.",
  },
  {
    id: "documents",
    title: "Документы",
    description: "Оригиналы файлов, которые вы храните в ВК.",
  },
  {
    id: "data",
    title: "Другие данные",
    description: "Профиль, записи и доступные данные в формате JSON.",
  },
];
const countSelection = (c: Collection, s?: Selection) =>
  s ? (s.itemIds?.length ?? Math.max(0, c.count - s.excludedIds.length)) : 0;
const selectedMedia = (m: Media, s?: Selection) =>
  !!s && (s.itemIds ? s.itemIds.includes(m.id) : !s.excludedIds.includes(m.id));
const errorText = (e: unknown) =>
  typeof e === "string"
    ? e
    : e instanceof Error
      ? e.message
      : "Не удалось выполнить действие. Попробуйте ещё раз.";

export default function App() {
  const [user, setUser] = useState<User | null>(null);
  const [sources, setSources] = useState<Source[]>([]);
  const [sourceId, setSourceId] = useState<number | null>(null);
  const [collections, setCollections] = useState<Record<number, Collection[]>>(
    {},
  );
  const [selections, setSelections] = useState<Record<string, Selection>>({});
  const [view, setView] = useState<View>("photos");
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [scanText, setScanText] = useState("Читаем данные вашего аккаунта…");
  const [error, setError] = useState<string | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const [loginOpen, setLoginOpen] = useState(false);
  const [loginTab, setLoginTab] = useState("oauth");
  const [miniappId, setMiniappId] = useState(
    localStorage.getItem("vk-backup-miniapp") ||
      import.meta.env.VITE_VK_MINIAPP_ID ||
      "54816788",
  );
  const [pairCode, setPairCode] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [token, setToken] = useState("");
  const [clientId, setClientId] = useState(
    localStorage.getItem("vk-backup-client") ||
      import.meta.env.VITE_VK_CLIENT_ID ||
      "54816594",
  );
  const [mode, setMode] = useState("vkid");
  const [scopes, setScopes] = useState("photos video docs groups wall friends");
  const [loginWaiting, setLoginWaiting] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [folder, setFolder] = useState<string | null>(null);
  const [useYtdlp, setUseYtdlp] = useState(
    localStorage.getItem("vk-backup-ytdlp") === "on",
  );
  const [hasYtdlp, setHasYtdlp] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [starting, setStarting] = useState(false);
  const [preview, setPreview] = useState<Collection | null>(null);
  const [previewItems, setPreviewItems] = useState<Media[]>([]);
  const [previewOffset, setPreviewOffset] = useState<number | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [showErrors, setShowErrors] = useState(false);
  const previewGeneration = useRef(0);
  const busy = isBusy(progress);
  const locked = busy || starting || loading;
  const activeSource = sources.find((s) => s.id === sourceId);
  const activeCollections = sourceId ? (collections[sourceId] ?? []) : [];
  const allCollections = useMemo(
    () => Object.values(collections).flat(),
    [collections],
  );
  const selectedCollections = allCollections.filter(
    (c) => countSelection(c, selections[c.id]) > 0,
  );
  const selectedCount = selectedCollections.reduce(
    (n, c) => n + countSelection(c, selections[c.id]),
    0,
  );
  const selectedSources = new Set(selectedCollections.map((c) => c.ownerId))
    .size;
  const activeView = views.find((v) => v.id === view)!;
  const visible = activeCollections.filter(
    (c) =>
      (view === "data" ? !isMedia(c.kind) : c.kind === view) &&
      `${c.title} ${c.description}`
        .toLocaleLowerCase()
        .includes(query.toLocaleLowerCase()),
  );
  const selectable = visible.filter((c) => c.available && c.count > 0);
  const checkedCount = selectable.filter(
    (c) => countSelection(c, selections[c.id]) > 0,
  ).length;

  async function scanSource(id: number, force = false) {
    setSourceId(id);
    setQuery("");
    setError(null);
    if (collections[id] && !force) return;
    setLoading(true);
    setScanText("Читаем альбомы и проверяем доступ…");
    try {
      const list = await backend.catalog(id);
      setCollections((old) => ({ ...old, [id]: list }));
      if (force)
        setSelections((old) =>
          Object.fromEntries(
            Object.entries(old).filter(([key]) => !key.startsWith(`${id}:`)),
          ),
        );
    } catch (e) {
      setError(errorText(e));
    } finally {
      setLoading(false);
    }
  }

  async function connected(u: User) {
    setUser(u);
    setToken("");
    setLoginOpen(false);
    setLoginWaiting(false);
    setPairCode("");
    setConnecting(false);
    setCollections({});
    setSelections({});
    setLoading(true);
    setError(null);
    setWarning(null);
    try {
      const result = await backend.sources();
      setSources(result.items);
      setWarning(result.warning);
      setSourceId(u.id);
      const list = await backend.catalog(u.id);
      setCollections({ [u.id]: list });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    let disposed = false;
    const removers: (() => void)[] = [];
    const add = (stop: () => void) => (disposed ? stop() : removers.push(stop));
    if (native || demo) {
      backend
        .progress((p) => {
          if (!disposed) setProgress(p);
        })
        .then(add)
        .catch((e) => setError(errorText(e)));
      backend.tools().then((v) => {
        if (!disposed) {
          setHasYtdlp(v);
          if (localStorage.getItem("vk-backup-ytdlp") === null) setUseYtdlp(v);
        }
      });
    }
    if (native) {
      listen<string>("scan-progress", (e) => setScanText(e.payload)).then(add);
      listen<{ user?: User; error?: string }>("auth-result", (e) => {
        if (disposed) return;
        setLoginWaiting(false);
        setPairCode("");
        if (e.payload.user) void connected(e.payload.user);
        else if (e.payload.error !== "Вход отменён.")
          setError(e.payload.error ?? "Не удалось войти в ВК.");
      }).then(add);
    }
    if (demo)
      backend.connect("demo").then((u) => {
        if (!disposed) void connected(u);
      });
    return () => {
      disposed = true;
      removers.forEach((fn) => fn());
    };
    // The subscription spans the lifetime of the app. connected uses only fresh API results.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!busy) return;
    const timer = setInterval(() => {
      backend
        .status()
        .then((p) => {
          if (p) setProgress(p);
        })
        .catch(() => {});
    }, 2000);
    return () => clearInterval(timer);
  }, [busy]);

  const toggleCollection = (c: Collection, checked: boolean) =>
    setSelections((old) => {
      const next = { ...old };
      if (checked)
        next[c.id] = { collectionId: c.id, itemIds: null, excludedIds: [] };
      else delete next[c.id];
      return next;
    });

  function toggleItem(c: Collection, m: Media, checked: boolean) {
    setSelections((old) => {
      const s = old[c.id] ?? {
        collectionId: c.id,
        itemIds: [],
        excludedIds: [],
      };
      if (s.itemIds === null)
        return {
          ...old,
          [c.id]: {
            ...s,
            excludedIds: checked
              ? s.excludedIds.filter((id) => id !== m.id)
              : [...new Set([...s.excludedIds, m.id])],
          },
        };
      return {
        ...old,
        [c.id]: {
          ...s,
          itemIds: checked
            ? [...new Set([...s.itemIds, m.id])]
            : s.itemIds.filter((id) => id !== m.id),
        },
      };
    });
  }

  async function loadPreview(c: Collection, offset = 0) {
    const generation =
      offset === 0 ? ++previewGeneration.current : previewGeneration.current;
    if (offset === 0) {
      setPreview(c);
      setPreviewItems([]);
      setPreviewOffset(null);
    }
    setPreviewLoading(true);
    setPreviewError(null);
    try {
      const page = await backend.items(c, offset);
      if (generation !== previewGeneration.current) return;
      setPreviewItems((old) => (offset ? [...old, ...page.items] : page.items));
      setPreviewOffset(page.nextOffset);
    } catch (e) {
      if (generation === previewGeneration.current)
        setPreviewError(errorText(e));
    } finally {
      if (generation === previewGeneration.current) setPreviewLoading(false);
    }
  }

  async function connectToken() {
    setConnecting(true);
    setError(null);
    try {
      await connected(await backend.connect(token));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setConnecting(false);
    }
  }
  async function login() {
    setError(null);
    setLoginWaiting(true);
    localStorage.setItem("vk-backup-client", clientId);
    try {
      await backend.login(
        clientId,
        mode,
        mode === "legacy" ? scopes.replace(/ +/g, ",") : scopes,
      );
    } catch (e) {
      setError(errorText(e));
      setLoginWaiting(false);
    }
  }
  async function cancelLogin() {
    try {
      await backend.cancelLogin();
      setLoginWaiting(false);
      setPairCode("");
    } catch (e) {
      setError(errorText(e));
    }
  }
  async function chooseFolder() {
    try {
      const path = await backend.folder();
      if (path) setFolder(path);
    } catch (e) {
      setError(errorText(e));
    }
  }
  async function miniappLogin() {
    setError(null);
    setPairCode("");
    setLoginWaiting(true);
    localStorage.setItem("vk-backup-miniapp", miniappId);
    try {
      const pair = await backend.miniappLogin(miniappId, "photos video groups");
      setPairCode(pair.pairCode);
    } catch (e) {
      setError(errorText(e));
      setLoginWaiting(false);
    }
  }
  async function start() {
    if (!folder) return;
    setStarting(true);
    setError(null);
    setShowErrors(false);
    try {
      const result = await backend.start(
        folder,
        selectedCollections.map((c) => selections[c.id]),
        useYtdlp,
      );
      setProgress((old) => (old?.id === result.id ? old : result));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setStarting(false);
    }
  }
  async function control(action: string) {
    if (progress)
      try {
        await backend.control(progress.id, action);
      } catch (e) {
        setError(errorText(e));
      }
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-symbol">
            <Archive size={23} />
          </span>
          <span>
            VK Backup<small>Всё важное — у вас</small>
          </span>
        </div>
        <div className="sidebar-label">БИБЛИОТЕКА</div>
        <nav className="main-nav" aria-label="Типы данных">
          {views.map((v) => {
            const Icon = v.id === "data" ? FileJson : icons[v.id];
            const count = activeCollections
              .filter((c) =>
                v.id === "data"
                  ? !isMedia(c.kind) && c.available
                  : c.kind === v.id && c.available,
              )
              .reduce((n, c) => n + (v.id === "photos" ? 1 : c.count), 0);
            return (
              <button
                className={`nav-item ${view === v.id ? "active" : ""}`}
                key={v.id}
                onClick={() => {
                  setView(v.id);
                  setQuery("");
                }}
              >
                <Icon size={19} />
                <span>{v.title}</span>
                {user && count > 0 && (
                  <span className="nav-count">{formatCount(count)}</span>
                )}
              </button>
            );
          })}
        </nav>
        <div className="sidebar-divider" />
        <div className="sidebar-label source-label">
          ИСТОЧНИКИ{" "}
          {user && (
            <button
              className="icon-button"
              title="Обновить сообщества"
              aria-label="Обновить сообщества"
              disabled={locked}
              onClick={async () => {
                setLoading(true);
                try {
                  const r = await backend.sources();
                  setSources(r.items);
                  setWarning(r.warning);
                } catch (e) {
                  setError(errorText(e));
                } finally {
                  setLoading(false);
                }
              }}
            >
              <RefreshCw size={13} />
            </button>
          )}
        </div>
        <div className="source-list">
          {sources.length ? (
            sources.map((source) => (
              <button
                key={source.id}
                disabled={locked}
                className={`source-button ${source.id === sourceId ? "chosen" : ""}`}
                onClick={() => void scanSource(source.id)}
              >
                <span
                  className={`source-avatar ${source.community ? "group" : ""}`}
                >
                  {source.avatar ? (
                    <img
                      src={source.avatar}
                      alt=""
                      referrerPolicy="no-referrer"
                    />
                  ) : source.community ? (
                    <UsersRound size={17} />
                  ) : (
                    <span>{source.name.slice(0, 1)}</span>
                  )}
                </span>
                <span className="source-name">
                  {source.community ? source.name : "Мой аккаунт"}
                  <small>
                    {source.community ? "Администратор" : source.name}
                  </small>
                </span>
                {selectedCollections.some((c) => c.ownerId === source.id) && (
                  <span className="selection-dot" />
                )}
              </button>
            ))
          ) : (
            <div className="sources-placeholder">
              <UsersRound size={19} />
              <p>Аккаунт и ваши сообщества появятся после входа</p>
            </div>
          )}
        </div>
        <div className="sidebar-bottom">
          <div className="privacy-note">
            <ShieldCheck size={22} />
            <span>
              Остаётся на компьютере<small>Без облака и чужих серверов</small>
            </span>
          </div>
          <button className="nav-item" onClick={() => setSettingsOpen(true)}>
            <Settings2 size={18} />
            Настройки
          </button>
          <button className="nav-item" onClick={() => setHelpOpen(true)}>
            <CircleHelp size={18} />
            Как это работает
          </button>
          <div className="version">
            VK Backup <span>v0.1.0</span>
          </div>
        </div>
      </aside>

      <main className="main">
        <header className="topbar">
          <span className="breadcrumb">
            <HardDrive size={15} /> Локальный архив <ChevronRight size={13} />
            <strong>
              {activeSource?.community ? activeSource.name : "Мой аккаунт"}
            </strong>
          </span>
          <div className="topbar-right">
            {demo && (
              <span className="demo-badge">Демо · файлы не скачиваются</span>
            )}
            <span className="local-badge">
              <span /> Локально
            </span>
            {user ? (
              <button
                className="icon-button"
                title="Выйти из аккаунта"
                aria-label="Выйти из аккаунта"
                disabled={locked}
                onClick={async () => {
                  try {
                    await backend.logout();
                    setUser(null);
                    setSources([]);
                    setCollections({});
                    setSelections({});
                    setSourceId(null);
                    setProgress(null);
                  } catch (e) {
                    setError(errorText(e));
                  }
                }}
              >
                <LogOut size={16} />
              </button>
            ) : (
              <button
                className="button small"
                onClick={() => setLoginOpen(true)}
                disabled={!native}
              >
                Войти в ВК <ArrowRight size={14} />
              </button>
            )}
          </div>
        </header>

        <div className="main-scroll">
          {error && (
            <div className="notice error" role="alert">
              <AlertCircle size={18} />
              <span>{error}</span>
              <button
                className="icon-button"
                aria-label="Скрыть ошибку"
                onClick={() => setError(null)}
              >
                <X size={16} />
              </button>
            </div>
          )}
          {warning && (
            <div className="notice warning" role="status">
              <Info size={18} />
              <span>{warning}</span>
              <button
                className="icon-button"
                aria-label="Скрыть уведомление"
                onClick={() => setWarning(null)}
              >
                <X size={16} />
              </button>
            </div>
          )}

          {!user ? (
            <section className="welcome">
              <div className="welcome-copy">
                <div className="eyebrow">
                  <span /> ВАШ ЛИЧНЫЙ АРХИВ
                </div>
                <h1>
                  Воспоминания.
                  <br />
                  Теперь у вас.
                </h1>
                <p>
                  Сохраните фотографии, видео и документы из ВК на свой
                  компьютер. Вместе с альбомами и данными ваших сообществ.
                </p>
                <button
                  className="button primary large"
                  disabled={!native}
                  onClick={() => setLoginOpen(true)}
                >
                  <span className="vk-mark">VK</span> Подключить ВК{" "}
                  <ArrowRight size={18} />
                </button>
                {!native && (
                  <a className="preview-link" href="/?demo=1">
                    Посмотреть демонстрацию интерфейса <ArrowRight size={15} />
                  </a>
                )}
                <span className="welcome-trust">
                  <LockKeyhole size={14} /> Пароль вводится только на стороне ВК
                </span>
              </div>
              <div className="welcome-art" aria-hidden="true">
                <div className="art-circle" />
                <div className="art-photo first">
                  <img src="/demo/coast.svg" />
                  <span>
                    Те самые летние дни <small>48 фотографий</small>
                  </span>
                </div>
                <div className="art-photo second">
                  <img src="/demo/mountain.svg" />
                  <span>
                    Выше облаков <small>36 фотографий</small>
                  </span>
                </div>
                <div className="art-saved">
                  <span>
                    <Check size={16} />
                  </span>{" "}
                  Сохранено у вас
                </div>
                <div className="art-spark">
                  <Sparkles size={27} />
                </div>
              </div>
              <div className="welcome-features">
                {[
                  {
                    icon: LayoutGrid,
                    title: "Порядок сохранён",
                    text: "Фотографии останутся в своих альбомах",
                  },
                  {
                    icon: UsersRound,
                    title: "Включая сообщества",
                    text: "Личный аккаунт и группы, где вы администратор",
                  },
                  {
                    icon: Download,
                    title: "Под вашим контролем",
                    text: "Выберите файлы, папку и время для бэкапа",
                  },
                ].map((f) => (
                  <div key={f.title}>
                    <f.icon size={22} />
                    <strong>{f.title}</strong>
                    <p>{f.text}</p>
                  </div>
                ))}
              </div>
            </section>
          ) : (
            <>
              <section className="page-heading">
                <div>
                  <div className="eyebrow">
                    ВАША БИБЛИОТЕКА{" "}
                    {activeSource?.community && (
                      <span className="community-tag">Сообщество</span>
                    )}
                  </div>
                  <h1>{activeView.title}</h1>
                  <p>{activeView.description}</p>
                </div>
                <button
                  className="button subtle"
                  disabled={locked}
                  onClick={() => sourceId && void scanSource(sourceId, true)}
                >
                  <RefreshCw size={16} className={loading ? "spin" : ""} />
                  Обновить
                </button>
              </section>
              <section className="library-summary">
                <div>
                  <span className="summary-icon blue">
                    <Image size={20} />
                  </span>
                  <span>
                    <strong>
                      {formatCount(
                        activeCollections
                          .filter((c) => c.kind === "photos" && c.available)
                          .reduce((n, c) => n + c.count, 0),
                      )}
                    </strong>
                    <small>фотографий</small>
                  </span>
                </div>
                <div>
                  <span className="summary-icon purple">
                    <icons.videos size={20} />
                  </span>
                  <span>
                    <strong>
                      {formatCount(
                        activeCollections.find((c) =>
                          c.id.endsWith("videos:all"),
                        )?.count ?? 0,
                      )}
                    </strong>
                    <small>видеозаписей</small>
                  </span>
                </div>
                <div>
                  <span className="summary-icon amber">
                    <icons.documents size={20} />
                  </span>
                  <span>
                    <strong>
                      {formatCount(
                        activeCollections.find((c) => c.kind === "documents")
                          ?.count ?? 0,
                      )}
                    </strong>
                    <small>документов</small>
                  </span>
                </div>
                <span className="summary-caption">
                  <ShieldCheck size={17} /> Сохраняем доступные оригиналы
                </span>
              </section>

              <div className="library-toolbar">
                <label className="search-field">
                  <Search size={17} />
                  <input
                    aria-label="Поиск по библиотеке"
                    value={query}
                    onChange={(e) => setQuery(e.target.value)}
                    placeholder={
                      view === "photos" ? "Найти альбом…" : "Найти раздел…"
                    }
                  />
                </label>
                <div className="toolbar-selection">
                  <CheckBox
                    checked={
                      checkedCount === 0
                        ? false
                        : checkedCount === selectable.length
                          ? true
                          : "indeterminate"
                    }
                    onChange={(v) =>
                      selectable.forEach((c) => toggleCollection(c, v))
                    }
                    label="Выбрать все видимые разделы"
                    disabled={locked || !selectable.length}
                  />
                  <span>Выбрать всё</span>
                  <span className="toolbar-divider" />
                  <span>
                    {formatCount(visible.length)}{" "}
                    {view === "photos" ? "альбомов" : "разделов"}
                  </span>
                </div>
              </div>
              {loading ? (
                <div className="loading-library" role="status">
                  <LoaderCircle className="spin" size={30} />
                  <h3>{scanText}</h3>
                  <p>
                    Проверяем доступ и собираем список. Большие библиотеки могут
                    занять немного времени.
                  </p>
                  <Bar value={40} indeterminate label="Загрузка библиотеки" />
                </div>
              ) : visible.length === 0 ? (
                <div className="empty-state">
                  <Archive size={36} />
                  <h3>{query ? "Ничего не найдено" : "Здесь пока пусто"}</h3>
                  <p>
                    {query
                      ? "Попробуйте другое название."
                      : "ВК не вернул данных для этого раздела."}
                  </p>
                </div>
              ) : (
                <div className={view === "data" ? "data-list" : "album-grid"}>
                  {visible.map((c) => {
                    const Icon = icons[c.kind];
                    const n = countSelection(c, selections[c.id]);
                    const all = n === c.count;
                    if (view === "data")
                      return (
                        <article
                          className={`data-row ${!c.available ? "unavailable" : ""}`}
                          key={c.id}
                        >
                          <span className="data-icon">
                            <Icon size={23} />
                          </span>
                          <div>
                            <h3>{c.title}</h3>
                            <p>{c.available ? c.description : c.error}</p>
                          </div>
                          <span className="json-badge">JSON</span>
                          {!c.available ? (
                            <LockKeyhole size={18} />
                          ) : (
                            <CheckBox
                              label={`Выбрать ${c.title}`}
                              checked={n > 0}
                              disabled={locked}
                              onChange={(v) => toggleCollection(c, v)}
                            />
                          )}
                        </article>
                      );
                    return (
                      <article
                        className={`album-card ${n > 0 ? "selected" : ""} ${!c.available ? "unavailable" : ""}`}
                        key={c.id}
                      >
                        <button
                          className={`album-cover ${c.cover ? "" : `placeholder-cover ${c.kind}`}`}
                          disabled={!c.available || locked}
                          onClick={() => void loadPreview(c)}
                          aria-label={`Просмотреть ${c.title}`}
                        >
                          {c.cover ? (
                            <img
                              loading="lazy"
                              src={c.cover}
                              alt=""
                              referrerPolicy="no-referrer"
                            />
                          ) : (
                            <Icon size={40} strokeWidth={1.3} />
                          )}
                          {c.available ? (
                            <>
                              <span className="cover-count">
                                {formatCount(c.count)}{" "}
                                {c.kind === "photos" ? "фото" : "файлов"}
                              </span>
                              <span className="cover-open">
                                Открыть <ArrowRight size={14} />
                              </span>
                            </>
                          ) : (
                            <span className="cover-locked">
                              <LockKeyhole size={20} />
                              Доступ ограничен
                            </span>
                          )}
                          {n > 0 && (
                            <span className="cover-selected">
                              <Check size={15} />
                              {all ? "Весь альбом" : `${n} выбрано`}
                            </span>
                          )}
                        </button>
                        <div className="album-footer">
                          <div>
                            <button
                              className="album-title"
                              disabled={!c.available || locked}
                              onClick={() => void loadPreview(c)}
                              title={c.title}
                            >
                              {c.title}
                            </button>
                            <small>
                              {c.available
                                ? c.kind === "photos"
                                  ? "Оригинальный размер"
                                  : c.description
                                : "ВК не разрешил доступ к разделу"}
                            </small>
                          </div>
                          {c.available ? (
                            <CheckBox
                              checked={
                                n > 0 ? (all ? true : "indeterminate") : false
                              }
                              onChange={(v) => toggleCollection(c, v)}
                              label={`Выбрать ${c.title}`}
                              disabled={locked || c.count === 0}
                            />
                          ) : (
                            <button
                              className="icon-button"
                              title={c.error ?? "Доступ ограничен"}
                              aria-label={`Почему недоступно: ${c.title}`}
                              onClick={() => setWarning(c.error)}
                            >
                              <Info size={18} />
                            </button>
                          )}
                        </div>
                      </article>
                    );
                  })}
                </div>
              )}
              <div className="library-footnote">
                <Info size={14} /> Можно открыть альбом и выбрать отдельные
                файлы. Выбор сохраняется при переключении сообществ.
              </div>
            </>
          )}

          {progress && (
            <section
              className={`backup-panel ${busy ? "in-progress" : ""}`}
              aria-live="polite"
            >
              <div className="backup-panel-heading">
                <span className="backup-state-icon">
                  {busy ? (
                    progress.status === "paused" ? (
                      <Pause size={21} />
                    ) : (
                      <ArrowDownToLine size={21} />
                    )
                  ) : progress.status === "completed" ? (
                    <CircleCheck size={23} />
                  ) : (
                    <AlertCircle size={22} />
                  )}
                </span>
                <div>
                  <h3>
                    {
                      {
                        running:
                          progress.phase === "scanning"
                            ? "Готовим ваш бэкап"
                            : "Сохраняем ваши файлы",
                        paused: "Бэкап на паузе",
                        canceling: "Отменяем бэкап…",
                        completed: demo
                          ? "Демонстрация завершена"
                          : "Ваш бэкап готов",
                        partial: "Бэкап завершён с пропусками",
                        failed: "Бэкап остановлен из-за ошибки",
                        canceled: "Бэкап отменён",
                      }[progress.status]
                    }
                  </h3>
                  <p>
                    {progress.phase === "scanning"
                      ? "Читаем все страницы выбранных альбомов"
                      : `${formatCount(processed(progress))} из ${formatCount(progress.total)} объектов обработано`}
                  </p>
                </div>
                <span className="backup-percent">
                  {progress.phase !== "scanning" &&
                    `${Math.round(percent(progress))}%`}
                </span>
                {busy && (
                  <div className="backup-controls">
                    <button
                      className="button small"
                      disabled={progress.status === "canceling"}
                      onClick={() =>
                        void control(
                          progress.status === "paused" ? "resume" : "pause",
                        )
                      }
                    >
                      {progress.status === "paused" ? (
                        <Play size={14} />
                      ) : (
                        <Pause size={14} />
                      )}
                      {progress.status === "paused" ? "Продолжить" : "Пауза"}
                    </button>
                    <button
                      className="icon-button"
                      aria-label="Отменить бэкап"
                      disabled={progress.status === "canceling"}
                      onClick={() => void control("cancel")}
                    >
                      <X size={18} />
                    </button>
                  </div>
                )}
              </div>
              <Bar
                value={percent(progress)}
                indeterminate={progress.phase === "scanning"}
                label="Общий прогресс бэкапа"
              />
              {busy && (
                <div className="current-file">
                  <span>{progress.current}</span>
                  <span>{formatBytes(progress.speed)}/с</span>
                </div>
              )}
              {busy && progress.phase === "downloading" && (
                <Bar
                  className="file-progress"
                  value={
                    progress.currentTotal
                      ? (progress.currentBytes / progress.currentTotal) * 100
                      : 0
                  }
                  indeterminate={!progress.currentTotal}
                  label="Прогресс текущего файла"
                />
              )}
              <div className="backup-stats">
                <span>
                  <Check size={14} />
                  Сохранено: {progress.completed}
                </span>
                <span>
                  <CheckCheck size={15} />
                  Уже в архиве: {progress.skipped}
                </span>
                <span>
                  <HardDrive size={14} />
                  {formatBytes(progress.bytes)}
                </span>
                {progress.failed + progress.unavailable > 0 && (
                  <button
                    onClick={() => setShowErrors((v) => !v)}
                    className="error-link"
                  >
                    <AlertCircle size={14} />
                    Пропуски и ошибки: {progress.failed + progress.unavailable}
                  </button>
                )}
                {!busy && !demo && (
                  <button
                    className="folder-link"
                    onClick={() =>
                      backend.openFolder().catch((e) => setError(errorText(e)))
                    }
                  >
                    <FolderOpen size={15} />
                    Открыть папку
                  </button>
                )}
              </div>
              {showErrors && (
                <div className="error-report">
                  {progress.errors.map((e, i) => (
                    <p key={i}>{e}</p>
                  ))}
                </div>
              )}
              {!busy && (
                <p className="completion-note">
                  {demo
                    ? "Это симуляция. Файлы на диск не записывались."
                    : progress.status === "completed"
                      ? "В папке бэкапа: файлы, описания и index.html для просмотра без интернета."
                      : "Готовые файлы сохранены. Повторный запуск проверит архив и скачает оставшееся."}
                </p>
              )}
            </section>
          )}
        </div>

        <footer className="download-tray">
          <button
            className={`destination ${folder ? "has-folder" : ""}`}
            disabled={!user || locked}
            onClick={() => void chooseFolder()}
          >
            <span className="destination-icon">
              <Folder size={21} />
            </span>
            <span>
              <small>СОХРАНИТЬ В</small>
              <strong title={folder ?? ""}>
                {folder
                  ? folder.split(/[\\/]/).filter(Boolean).slice(-2).join(" / ")
                  : "Выберите папку на компьютере"}
              </strong>
            </span>
            <ChevronRight size={16} />
          </button>
          <div className="download-summary">
            <strong>
              {selectedCount
                ? `${formatCount(selectedCount)} выбрано`
                : "Выберите, что сохранить"}
            </strong>
            <small>
              {selectedCount
                ? `${selectedCollections.length} разделов · ${selectedSources} источников`
                : "Альбомы, видео и документы"}
            </small>
          </div>
          <button
            className="button primary download-button"
            disabled={!user || locked || !folder || selectedCount === 0}
            onClick={() => void start()}
          >
            {starting ? (
              <LoaderCircle className="spin" size={18} />
            ) : (
              <Download size={18} />
            )}
            {busy
              ? "Бэкап выполняется"
              : demo
                ? "Запустить демо"
                : "Создать бэкап"}
          </button>
        </footer>
      </main>

      <Modal
        open={loginOpen}
        onOpenChange={(v) => {
          if (!connecting) {
            setLoginOpen(v);
            if (!v && loginWaiting) void cancelLogin();
          }
        }}
        title="Подключить аккаунт ВК"
        description="Авторизация нужна для чтения ваших альбомов и списка сообществ."
      >
        {error && (
          <div className="notice error" role="alert">
            <AlertCircle size={16} />
            <span>{error}</span>
          </div>
        )}
        {!native ? (
          <p>
            Для подключения ВК запустите десктопное приложение.{" "}
            <a href="/?demo=1">Открыть демонстрацию</a>
          </p>
        ) : (
          <Tabs.Root
            value={loginTab}
            onValueChange={setLoginTab}
            className="auth-tabs"
          >
            <Tabs.List className="tab-list">
              <Tabs.Trigger value="oauth" disabled={loginWaiting || connecting}>
                Вход через браузер
              </Tabs.Trigger>
              <Tabs.Trigger value="token" disabled={loginWaiting || connecting}>
                У меня есть токен
              </Tabs.Trigger>
              <Tabs.Trigger
                value="miniapp"
                disabled={loginWaiting || connecting}
              >
                Через Mini App
              </Tabs.Trigger>
            </Tabs.List>
            <Tabs.Content value="oauth">
              <label className="field-label">
                ID приложения ВК
                <input
                  value={clientId}
                  onChange={(e) => setClientId(e.target.value)}
                  placeholder="Например, 12345678"
                  inputMode="numeric"
                  disabled={loginWaiting}
                />
              </label>
              <label className="field-label">
                Способ входа
                <select
                  value={mode}
                  disabled={loginWaiting}
                  onChange={(e) => setMode(e.target.value)}
                >
                  <option value="vkid">VK ID · OAuth 2.1 / PKCE</option>
                  <option value="legacy">
                    VK API · существующее Standalone-приложение
                  </option>
                </select>
              </label>
              <div className="setup-note">
                <Info size={17} />
                <p>
                  Для вашего приложения укажите адрес возврата:
                  <code>
                    {mode === "legacy"
                      ? "http://127.0.0.1:47851/callback"
                      : "https://vk-backup.whitesquirrel.digital/callback"}
                  </code>
                  VK ID может не предоставить права на медиа. Если это
                  произошло, подключите пользовательский токен VK API.
                </p>
              </div>
              <details className="scope-details">
                <summary>Запрашиваемые права</summary>
                <label className="field-label">
                  <input
                    aria-label="Права приложения ВК"
                    value={scopes}
                    disabled={loginWaiting}
                    onChange={(e) => setScopes(e.target.value)}
                  />
                </label>
                <small>
                  Для переписки нужен дополнительный доступ messages.
                  Возможность выдачи прав зависит от типа приложения ВК.
                </small>
              </details>
              {loginWaiting ? (
                <div className="waiting-auth">
                  <LoaderCircle className="spin" size={20} />
                  <span>Завершите вход в браузере…</span>
                  <button
                    className="button small"
                    onClick={() => void cancelLogin()}
                  >
                    Отменить
                  </button>
                </div>
              ) : (
                <button
                  className="button primary full"
                  disabled={!clientId || connecting}
                  onClick={() => void login()}
                >
                  <ExternalLink size={17} />
                  Открыть ВК и войти
                </button>
              )}
            </Tabs.Content>
            <Tabs.Content value="miniapp">
              <label className="field-label">
                ID VK Mini App
                <input
                  value={miniappId}
                  onChange={(e) => setMiniappId(e.target.value)}
                  inputMode="numeric"
                  placeholder="ID отдельного Mini App"
                  disabled={loginWaiting}
                />
              </label>
              <div className="setup-note">
                <Info size={17} />
                <p>
                  Mini App откроется внутри ВК. Подтвердите доступ к фото, видео
                  и группам. Приложение проверит разрешения перед подключением;
                  при отказе прежний вход сохранится.
                </p>
              </div>
              {loginWaiting ? (
                <>
                  <div className="waiting-auth">
                    <LoaderCircle className="spin" size={20} />
                    <span>Завершите подключение внутри ВК…</span>
                    <button
                      className="button small"
                      onClick={() => void cancelLogin()}
                    >
                      Отменить
                    </button>
                  </div>
                  {pairCode && (
                    <label className="field-label">
                      Если Mini App попросит код, скопируйте его отсюда
                      <input
                        aria-label="Код подключения Mini App"
                        readOnly
                        value={pairCode}
                        onFocus={(e) => e.target.select()}
                      />
                      <small>
                        Действует пять минут. Токен в код не входит.
                      </small>
                    </label>
                  )}
                </>
              ) : (
                <button
                  className="button primary full"
                  disabled={!miniappId || connecting}
                  onClick={() => void miniappLogin()}
                >
                  <ExternalLink size={17} />
                  Открыть Mini App в ВК
                </button>
              )}
            </Tabs.Content>
            <Tabs.Content value="token">
              <p className="muted">
                Нужен пользовательский токен с правами photos, video, docs и
                groups. Сервисный ключ и токен сообщества не подойдут.
              </p>
              <label className="field-label">
                Токен или ссылка после авторизации
                <textarea
                  autoComplete="off"
                  spellCheck={false}
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                  placeholder="vk1.a.… или ссылка с access_token"
                  rows={4}
                  disabled={connecting}
                />
              </label>
              <button
                className="button primary full"
                disabled={!token.trim() || connecting}
                onClick={() => void connectToken()}
              >
                {connecting ? (
                  <LoaderCircle className="spin" size={17} />
                ) : (
                  <LockKeyhole size={17} />
                )}
                Подключить аккаунт
              </button>
            </Tabs.Content>
          </Tabs.Root>
        )}
        <p className="auth-privacy">
          <ShieldCheck size={15} />
          Токен хранится только в памяти до закрытия приложения.
        </p>
      </Modal>

      <Modal
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        title="Настройки бэкапа"
        description="Данные сохраняются напрямую из ВК в выбранную папку."
      >
        <div className="settings-row">
          <div>
            <h3>Видео через yt-dlp</h3>
            <p>
              Использовать установленный yt-dlp, когда ВК не отдаёт прямую
              ссылку на видео.
            </p>
          </div>
          <CheckBox
            label="Использовать yt-dlp для видео"
            checked={useYtdlp}
            onChange={(enabled) => {
              setUseYtdlp(enabled);
              localStorage.setItem("vk-backup-ytdlp", enabled ? "on" : "off");
            }}
            disabled={busy}
          />
        </div>
        <div className={`tool-status ${hasYtdlp ? "available" : ""}`}>
          {hasYtdlp ? <CircleCheck size={16} /> : <Info size={16} />}
          {hasYtdlp
            ? "yt-dlp найден на компьютере"
            : "yt-dlp не найден. Для прямых файлов он не нужен."}
          <button
            className="text-button"
            onClick={() => backend.tools().then(setHasYtdlp)}
          >
            Проверить
          </button>
        </div>
        <div className="settings-info">
          {native && (
            <>
              <h3>Доступ к фото и сообществам</h3>
              <p>
                Можно подключиться через VK Mini App. Прежняя сессия
                сохраняется, пока новый доступ не будет проверен.
              </p>
              <button
                className="button"
                disabled={locked}
                onClick={() => {
                  setSettingsOpen(false);
                  setLoginTab("miniapp");
                  setError(null);
                  setLoginOpen(true);
                }}
              >
                Подключить через Mini App
              </button>
            </>
          )}
          <h3>Что входит в архив</h3>
          <p>
            Файлы по альбомам, сведения о сообществах и выбранные данные в JSON.
            Вложения стены и переписки сохраняются как данные; их медиа нужно
            выбирать в соответствующих разделах.
          </p>
          <h3>Повторный запуск</h3>
          <p>
            Готовые файлы проверяются по размеру и контрольной сумме. Они
            пропускаются, повреждённые скачиваются заново. Данные JSON
            обновляются.
          </p>
        </div>
      </Modal>

      <Modal
        open={helpOpen}
        onOpenChange={setHelpOpen}
        title="Ваш архив за три шага"
        description="Можно сохранить личный аккаунт и несколько сообществ за один запуск."
      >
        <ol className="help-steps">
          <li>
            <span>1</span>
            <div>
              <h3>Подключите ВК</h3>
              <p>
                Войдите через своё приложение ВК или вставьте пользовательский
                токен. Права каждого раздела проверяются отдельно.
              </p>
            </div>
          </li>
          <li>
            <span>2</span>
            <div>
              <h3>Выберите ваши данные</h3>
              <p>
                Отметьте альбомы, откройте их для выбора отдельных файлов.
                Переключайте источники, чтобы добавить группы, где вы
                администратор.
              </p>
            </div>
          </li>
          <li>
            <span>3</span>
            <div>
              <h3>Укажите папку и сохраните</h3>
              <p>
                Следите за прогрессом, ставьте загрузку на паузу. Откройте
                index.html в папке архива для просмотра без интернета.
              </p>
            </div>
          </li>
        </ol>
        <div className="setup-note">
          <Info size={17} />
          <p>
            ВК может ограничивать доступ к отдельным видео, приватным данным и
            переписке. Приложение сохраняет доступные файлы и показывает
            пропуски в отчёте. Музыка не включена.
          </p>
        </div>
      </Modal>

      <Modal
        open={!!preview}
        onOpenChange={(v) => {
          if (!v) {
            ++previewGeneration.current;
            setPreview(null);
          }
        }}
        title={preview?.title ?? "Альбом"}
        description={
          preview
            ? `${formatCount(preview.count)} файлов · выбрано ${formatCount(countSelection(preview, selections[preview.id]))}`
            : ""
        }
        wide
      >
        {preview && (
          <>
            <div className="preview-toolbar">
              <button
                className="button small"
                onClick={() => toggleCollection(preview, true)}
              >
                <CheckCheck size={15} />
                Выбрать весь альбом
              </button>
              <button
                className="button small subtle"
                onClick={() => toggleCollection(preview, false)}
              >
                Снять выбор
              </button>
              <span>Выбор применяется ко всем страницам</span>
            </div>
            {previewError && (
              <div className="notice error" role="alert">
                {previewError}
                <button
                  className="button small"
                  onClick={() => void loadPreview(preview)}
                >
                  Повторить
                </button>
              </div>
            )}
            <div className="preview-grid">
              {previewItems.map((m) => (
                <label
                  className={`preview-item ${selectedMedia(m, selections[preview.id]) ? "selected" : ""}`}
                  key={m.id}
                >
                  {m.thumbnail ? (
                    <img
                      src={m.thumbnail}
                      alt={m.title}
                      loading="lazy"
                      referrerPolicy="no-referrer"
                    />
                  ) : (
                    <span className="preview-file-icon">
                      <icons.documents size={30} />
                    </span>
                  )}
                  <span className="preview-check">
                    <CheckBox
                      checked={selectedMedia(m, selections[preview.id])}
                      onChange={(v) => toggleItem(preview, m, v)}
                      label={`Выбрать ${m.title}`}
                    />
                  </span>
                  {m.duration && (
                    <span className="duration">
                      {Math.floor(m.duration / 60)}:
                      {String(m.duration % 60).padStart(2, "0")}
                    </span>
                  )}
                  <span className="preview-caption" title={m.title}>
                    {m.title}
                  </span>
                  {m.reason && (
                    <span className="media-warning" title={m.reason}>
                      <Info size={14} />
                      {preview.kind === "videos" && useYtdlp && hasYtdlp
                        ? "Скачивание через видеостраницу"
                        : "Прямой файл недоступен"}
                    </span>
                  )}
                </label>
              ))}
            </div>
            {previewLoading && (
              <div className="preview-loading" role="status">
                <LoaderCircle className="spin" size={23} />
                Загружаем файлы…
              </div>
            )}
            {previewOffset !== null && !previewLoading && (
              <button
                className="button load-more"
                onClick={() => void loadPreview(preview, previewOffset)}
              >
                Показать ещё <ChevronRight size={15} />
              </button>
            )}
            <div className="preview-bottom">
              <span>
                <Clock3 size={15} /> Максимальное доступное качество
              </span>
              <button
                className="button primary"
                onClick={() => setPreview(null)}
              >
                Готово <Check size={16} />
              </button>
            </div>
          </>
        )}
      </Modal>
    </div>
  );
}
