export const LOOPBACK_REDIRECT = "http://127.0.0.1:47851/callback";

// Only an OAuth code response goes through the public page. Tokens are exchanged in Rust.
export function localCallback(input) {
  const source = new URL(input);
  const params = source.searchParams;
  if (
    source.hash ||
    params.has("access_token") ||
    params.has("refresh_token")
  ) {
    throw new Error(
      "Ожидался ответ VK ID с кодом авторизации. Повторите вход из приложения.",
    );
  }
  const take = (key, max) => {
    const values = params.getAll(key);
    if (
      values.length !== 1 ||
      !values[0] ||
      values[0].length > max ||
      /[\x00-\x1f\x7f]/.test(values[0])
    ) {
      throw new Error(
        "Ответ авторизации неполный. Повторите вход из VK Backup.",
      );
    }
    return values[0];
  };
  const state = take("state", 64);
  if (!/^[a-f0-9]{64}$/.test(state)) {
    throw new Error(
      "Не удалось проверить ответ авторизации. Повторите вход из VK Backup.",
    );
  }
  const target = new URL(LOOPBACK_REDIRECT);
  target.searchParams.set("state", state);
  if (params.has("error")) {
    if (params.has("code") || params.has("device_id")) {
      throw new Error(
        "Получен противоречивый ответ авторизации. Повторите вход.",
      );
    }
    target.searchParams.set("error", take("error", 512));
  } else {
    target.searchParams.set("code", take("code", 4096));
    target.searchParams.set("device_id", take("device_id", 4096));
  }
  // A caller-supplied destination is deliberately never used.
  return target.href;
}
