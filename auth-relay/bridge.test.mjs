import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { createBridgeServer } from "./bridge-server.mjs";
import {
  encryptResult,
  encode,
  decode,
  PROTOCOL,
} from "./public/bridge-crypto.js";

const fakeToken = "test-only-not-a-real-token-123456789";
async function setup(t, options) {
  const server = createBridgeServer(options);
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const base = `http://127.0.0.1:${server.address().port}/bridge`;
  const receiver = await crypto.subtle.generateKey(
    { name: "ECDH", namedCurve: "P-256" },
    true,
    ["deriveBits"],
  );
  const secret = randomBytes(32).toString("hex");
  const poll = {
    method: "POST",
    headers: { Authorization: `Bearer ${secret}` },
  };
  const publicKey = encode(
    await crypto.subtle.exportKey("raw", receiver.publicKey),
  );
  const creation = {
    receiverPublicKey: publicKey,
    pollSecretHash: createHash("sha256").update(secret).digest("hex"),
    appId: 12345,
    scopes: ["photos", "video", "groups"],
    expectedUserId: 123,
  };
  const request = (path, method = "GET", body, headers = {}) =>
    fetch(base + path, {
      method,
      headers: { "Content-Type": "application/json", ...headers },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
  const pair = await (await request("/sessions", "POST", creation)).json();
  return { request, receiver, secret, poll, creation, pair, base };
}
async function decrypt(receiver, pairId, envelope) {
  const remote = await crypto.subtle.importKey(
    "raw",
    decode(envelope.publicKey),
    { name: "ECDH", namedCurve: "P-256" },
    false,
    [],
  );
  const bits = await crypto.subtle.deriveBits(
    { name: "ECDH", public: remote },
    receiver.privateKey,
    256,
  );
  const material = await crypto.subtle.importKey("raw", bits, "HKDF", false, [
    "deriveKey",
  ]);
  const text = new TextEncoder();
  const key = await crypto.subtle.deriveKey(
    {
      name: "HKDF",
      hash: "SHA-256",
      salt: text.encode(pairId),
      info: text.encode(PROTOCOL),
    },
    material,
    { name: "AES-GCM", length: 256 },
    false,
    ["decrypt"],
  );
  return JSON.parse(
    new TextDecoder().decode(
      await crypto.subtle.decrypt(
        {
          name: "AES-GCM",
          iv: decode(envelope.nonce),
          additionalData: text.encode(pairId),
        },
        key,
        decode(envelope.ciphertext),
      ),
    ),
  );
}
test("encrypted handoff can be read only by its desktop and consumed once", async (t) => {
  const s = await setup(t);
  const path = `/sessions/${s.pair.pairId}`;
  const metadata = await s.request(path);
  assert.equal(metadata.headers.get("cache-control"), "no-store");
  const info = await metadata.json();
  assert.equal(info.appId, 12345);
  assert.ok(!JSON.stringify(info).includes(s.secret));
  assert.equal((await s.request(path + "/poll", "POST")).status, 403);
  assert.equal((await fetch(s.base + path + "/poll", s.poll)).status, 202);
  const envelope = await encryptResult(info.receiverPublicKey, s.pair.pairId, {
    accessToken: fakeToken,
    appId: 12345,
    userId: 123,
    scopes: info.scopes,
  });
  assert.ok(!JSON.stringify(envelope).includes(fakeToken));
  assert.equal(
    (await s.request(path + "/result", "POST", envelope)).status,
    201,
  );
  assert.equal(
    (await s.request(path + "/result", "POST", envelope)).status,
    201,
  ); // lost-response retry
  const modified = { ...envelope, nonce: encode(randomBytes(12)) };
  assert.equal(
    (await s.request(path + "/result", "POST", modified)).status,
    409,
  );
  const concurrent = await Promise.all([
    fetch(s.base + path + "/poll", s.poll),
    fetch(s.base + path + "/poll", s.poll),
  ]);
  assert.deepEqual(concurrent.map((r) => r.status).sort(), [200, 410]);
  const received = await concurrent.find((r) => r.status === 200).json();
  assert.equal(
    (await decrypt(s.receiver, s.pair.pairId, received)).accessToken,
    fakeToken,
  );
  await assert.rejects(decrypt(s.receiver, "f".repeat(64), received));
  assert.equal((await s.request(path)).status, 410);
});
test("expiration, cancellation, foreign origins and plaintext uploads are rejected", async (t) => {
  let time = 1_000;
  const s = await setup(t, { now: () => time });
  const path = `/sessions/${s.pair.pairId}`;
  assert.equal(
    (
      await s.request(path, "GET", undefined, {
        Origin: "https://evil.example",
      })
    ).status,
    403,
  );
  assert.equal(
    (await s.request(path + "/result", "POST", { access_token: fakeToken }))
      .status,
    400,
  );
  assert.equal(
    (await s.request("/sessions", "POST", { ...s.creation, scopes: ["audio"] }))
      .status,
    400,
  );
  assert.equal((await s.request(path, "DELETE")).status, 403);
  assert.equal(
    (
      await s.request(path, "DELETE", undefined, {
        Authorization: `Bearer ${s.secret}`,
      })
    ).status,
    200,
  );
  assert.equal((await s.request(path)).status, 410);
  const next = await (await s.request("/sessions", "POST", s.creation)).json();
  time += 300_001;
  assert.equal((await s.request(`/sessions/${next.pairId}`)).status, 410);
});
test("pairing server has bounded capacity and does not accept arbitrary request fields", async (t) => {
  const s = await setup(t, { maxSessions: 1 });
  assert.equal(
    (
      await s.request("/sessions", "POST", {
        ...s.creation,
        access_token: fakeToken,
      })
    ).status,
    400,
  );
  assert.equal((await s.request("/sessions", "POST", s.creation)).status, 429);
  assert.equal(
    (
      await s.request("/sessions", "POST", {
        ...s.creation,
        receiverPublicKey: "garbage",
      })
    ).status,
    400,
  );
});
