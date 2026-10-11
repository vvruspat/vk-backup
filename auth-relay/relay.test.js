import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { localCallback, LOOPBACK_REDIRECT } from "./public/relay.js";

const nonce = "a".repeat(64);
const publicCallback = "https://vk-backup.whitesquirrel.digital/callback";
const callback = (query) => `${publicCallback}?${new URLSearchParams(query)}`;

describe("HTTPS OAuth callback relay", () => {
  it("forwards encoded code responses only to the fixed loopback listener", () => {
    const target = new URL(
      localCallback(
        callback({
          state: nonce,
          code: "a+b&=c",
          device_id: "device",
          redirect_uri: "https://attacker.example",
          email: "discard@example.com",
        }),
      ),
    );
    expect(target.origin + target.pathname).toBe(LOOPBACK_REDIRECT);
    expect(Object.fromEntries(target.searchParams)).toEqual({
      state: nonce,
      code: "a+b&=c",
      device_id: "device",
    });
  });
  it("returns provider denial to the app without a token exchange", () => {
    const target = new URL(
      localCallback(callback({ state: nonce, error: "access_denied" })),
    );
    expect(Object.fromEntries(target.searchParams)).toEqual({
      state: nonce,
      error: "access_denied",
    });
  });
  it("rejects incomplete, ambiguous and oversized responses", () => {
    for (const input of [
      callback({ code: "code", device_id: "device" }),
      callback({ state: "wrong", code: "code", device_id: "device" }),
      callback({ state: nonce, code: "code" }),
      callback({ state: nonce, code: "a".repeat(4097), device_id: "device" }),
      callback({ state: nonce, code: "code", device_id: "device" }) +
        "&state=" +
        nonce,
      callback({
        state: nonce,
        code: "code",
        device_id: "device",
        error: "denied",
      }),
    ])
      expect(() => localCallback(input)).toThrow();
  });
  it("never relays access or refresh tokens", () => {
    for (const input of [
      callback({
        state: nonce,
        code: "code",
        device_id: "device",
        access_token: "secret",
      }),
      callback({
        state: nonce,
        code: "code",
        device_id: "device",
        refresh_token: "secret",
      }),
      publicCallback + "#access_token=secret",
    ])
      expect(() => localCallback(input)).toThrow();
  });
  it("uses the same HTTPS and loopback addresses as the native OAuth flow", () => {
    const rust = readFileSync(
      resolve(process.cwd(), "src-tauri/src/auth.rs"),
      "utf8",
    );
    expect(rust).toContain(`pub const REDIRECT: &str = "${publicCallback}";`);
    expect(rust).toContain(
      `pub const LOOPBACK_REDIRECT: &str = "${LOOPBACK_REDIRECT}";`,
    );
  });
});
