import bridge from "@vkontakte/vk-bridge";
import { encryptResult, validPairId } from "./public/bridge-crypto.js";

const element = (id) => document.getElementById(id);
const status = (text, error = false) => {
  element("status").textContent = text;
  element("status").dataset.error = String(error);
};
const appId = Number(new URLSearchParams(location.search).get("vk_app_id"));
const initialPair =
  new URLSearchParams(location.hash.slice(1)).get("pair") || "";
// Pairing capabilities remain in memory after loading, rather than in browser history.
history.replaceState(null, "", location.pathname + location.search);
let pairId = "";
let metadata = null;
let user = null;
let pendingEnvelope = null;
let expirationTimer = null;
const titles = {
  photos: "фотографии",
  video: "видеозаписи",
  groups: "администрируемые сообщества",
  docs: "документы",
  wall: "стена",
  friends: "друзья",
};

async function api(path, options = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 15_000);
  try {
    const response = await fetch(`/bridge${path}`, {
      ...options,
      cache: "no-store",
      credentials: "omit",
      redirect: "error",
      signal: controller.signal,
    });
    if (!response.ok)
      throw new Error(
        response.status === 410
          ? "expired"
          : response.status === 409
            ? "already_sent"
            : "server",
      );
    return await response.json();
  } finally {
    clearTimeout(timer);
  }
}
async function loadPair(value) {
  element("connect").disabled = true;
  pendingEnvelope = null;
  metadata = null;
  if (!validPairId(value)) throw new Error("invalid_pair");
  if (!bridge.isEmbedded() || !Number.isSafeInteger(appId) || appId <= 0)
    throw new Error("outside_vk");
  const info = await api(`/sessions/${value}`);
  if (info.appId !== appId) throw new Error("app_mismatch");
  if (info.expectedUserId !== null && info.expectedUserId !== user.id)
    throw new Error("account_mismatch");
  pairId = value;
  metadata = info;
  element("pair").value = "";
  element("pair-form").hidden = true;
  element("account").textContent = `${user.first_name} ${user.last_name}`;
  element("permissions").textContent =
    `Запрашиваем доступ: ${info.scopes.map((s) => titles[s]).join(", ")}.`;
  element("connect").hidden = false;
  element("connect").disabled = false;
  status("Подтвердите доступ к выбранным разделам ВК.");
  clearTimeout(expirationTimer);
  expirationTimer = setTimeout(
    () => {
      element("connect").disabled = true;
      status("Код подключения истёк. Откройте вход заново в VK Backup.", true);
    },
    Math.max(0, info.expiresAt - Date.now()),
  );
}
function showError(error) {
  // Never render raw VK error objects: request_params may contain an access token.
  const messages = {
    outside_vk:
      "Откройте Mini App внутри ВК по ссылке из приложения на компьютере.",
    invalid_pair: "Вставьте полный код подключения из приложения VK Backup.",
    expired: "Код подключения истёк. Откройте вход заново в VK Backup.",
    already_sent: "Ответ уже отправлен. Вернитесь в приложение на компьютере.",
    app_mismatch:
      "ID этого Mini App не совпадает с ID в приложении на компьютере.",
    account_mismatch:
      "Вы вошли в другой аккаунт ВК. Используйте аккаунт, подключённый в VK Backup.",
    missing_scopes:
      "ВК не предоставил все запрошенные разрешения. Подключение не завершено.",
    invalid_token:
      "ВК не вернул пользовательский токен. Подключение не завершено.",
  };
  status(
    messages[error?.message] ||
      "Не удалось завершить подключение. Проверьте интернет или повторите разрешение доступа.",
    true,
  );
}
element("pair-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  try {
    await loadPair(element("pair").value.trim());
  } catch (error) {
    showError(error);
  }
});
element("connect").addEventListener("click", async () => {
  if (!metadata) return;
  element("connect").disabled = true;
  status(
    pendingEnvelope
      ? "Повторяем отправку зашифрованного ответа…"
      : "Подтвердите разрешения в окне ВК…",
  );
  try {
    if (!pendingEnvelope) {
      const result = await bridge.send("VKWebAppGetAuthToken", {
        app_id: appId,
        scope: metadata.scopes.join(","),
      });
      const scopes =
        typeof result.scope === "string"
          ? result.scope
              .split(",")
              .map((s) => s.trim())
              .filter(Boolean)
          : [];
      if (metadata.scopes.some((s) => !scopes.includes(s)))
        throw new Error("missing_scopes");
      if (
        typeof result.access_token !== "string" ||
        result.access_token.length < 20 ||
        result.access_token.length > 8192
      )
        throw new Error("invalid_token");
      pendingEnvelope = await encryptResult(
        metadata.receiverPublicKey,
        pairId,
        { accessToken: result.access_token, appId, userId: user.id, scopes },
      );
      // Keep only ciphertext for a transport retry. The token is never shown or persisted.
    }
    await api(`/sessions/${pairId}/result`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(pendingEnvelope),
    });
    pendingEnvelope = null;
    clearTimeout(expirationTimer);
    element("connect").hidden = true;
    status(
      "Ответ отправлен. Вернитесь в VK Backup — приложение проверит доступ к вашим данным.",
    );
  } catch (error) {
    showError(error);
    element("connect").disabled = false;
  }
});

try {
  if (!bridge.isEmbedded() || !Number.isSafeInteger(appId) || appId <= 0)
    throw new Error("outside_vk");
  await bridge.send("VKWebAppInit");
  user = await bridge.send("VKWebAppGetUserInfo");
  if (validPairId(initialPair)) await loadPair(initialPair);
  else status("Вставьте код подключения из VK Backup на компьютере.");
} catch (error) {
  showError(error);
}
