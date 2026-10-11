import http from "node:http";
import { randomBytes, createHash, timingSafeEqual } from "node:crypto";
import { pathToFileURL } from "node:url";

const hex = (s) => typeof s === "string" && /^[a-f0-9]{64}$/.test(s);
const b64 = (s, min, max) =>
  typeof s === "string" &&
  s.length >= min &&
  s.length <= max &&
  /^[A-Za-z0-9_-]+$/.test(s);
const publicKey = (s) =>
  b64(s, 87, 87) &&
  Buffer.from(s, "base64url").length === 65 &&
  Buffer.from(s, "base64url")[0] === 4;
const allowedScopes = new Set([
  "photos",
  "video",
  "groups",
  "docs",
  "wall",
  "friends",
]);
const digest = (s) => createHash("sha256").update(s).digest();

// This process retains only ciphertext and public keys. It never writes requests to disk.
export function createBridgeServer({
  now = Date.now,
  ttlMs = 300_000,
  maxSessions = 256,
} = {}) {
  const sessions = new Map();
  const creationTimes = [];
  function cleanup() {
    const time = now();
    for (const [id, session] of sessions)
      if (session.expiresAt <= time) sessions.delete(id);
    while (creationTimes.length && creationTimes[0] <= time - 60_000)
      creationTimes.shift();
  }
  const timer = setInterval(cleanup, 15_000).unref();
  const reply = (res, code, data) => {
    res.writeHead(code, {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      "Referrer-Policy": "no-referrer",
      "X-Content-Type-Options": "nosniff",
      "Content-Security-Policy": "default-src 'none'; frame-ancestors 'none'",
    });
    res.end(JSON.stringify(data));
  };
  async function body(req) {
    if (
      !/^application\/json(?:\s*;|$)/i.test(req.headers["content-type"] || "")
    )
      throw new Error("body");
    const chunks = [];
    let size = 0;
    for await (const chunk of req) {
      size += chunk.length;
      if (size > 24_576) throw new Error("body");
      chunks.push(chunk);
    }
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  }
  function authorized(req, session) {
    const raw = req.headers.authorization || "";
    const secret = raw.startsWith("Bearer ") ? raw.slice(7) : "";
    return (
      hex(secret) && timingSafeEqual(digest(secret), session.pollSecretHash)
    );
  }
  const server = http.createServer(async (req, res) => {
    try {
      cleanup();
      const origin = req.headers.origin;
      if (origin && origin !== "https://vk-backup.whitesquirrel.digital")
        return reply(res, 403, { error: "origin" });
      if (req.method === "GET" && req.url === "/bridge/health")
        return reply(res, 200, { ok: true });
      if (req.method === "POST" && req.url === "/bridge/sessions") {
        const data = await body(req);
        if (
          !data ||
          Object.keys(data).some(
            (k) =>
              ![
                "receiverPublicKey",
                "pollSecretHash",
                "appId",
                "scopes",
                "expectedUserId",
              ].includes(k),
          ) ||
          !publicKey(data.receiverPublicKey) ||
          !hex(data.pollSecretHash) ||
          !Number.isSafeInteger(data.appId) ||
          data.appId <= 0 ||
          data.appId > 99_999_999_999 ||
          !Array.isArray(data.scopes) ||
          !data.scopes.length ||
          data.scopes.length > 6 ||
          data.scopes.some((s) => !allowedScopes.has(s)) ||
          new Set(data.scopes).size !== data.scopes.length ||
          (data.expectedUserId !== null &&
            (!Number.isSafeInteger(data.expectedUserId) ||
              data.expectedUserId <= 0))
        ) {
          return reply(res, 400, { error: "invalid_request" });
        }
        // Bound total memory and creation rate even if an untrusted caller rotates IP addresses.
        if (sessions.size >= maxSessions || creationTimes.length >= 60)
          return reply(res, 429, { error: "busy" });
        const pairId = randomBytes(32).toString("hex");
        const expiresAt = now() + ttlMs;
        sessions.set(pairId, {
          ...data,
          pollSecretHash: Buffer.from(data.pollSecretHash, "hex"),
          expiresAt,
          result: null,
        });
        creationTimes.push(now());
        return reply(res, 201, { pairId, expiresAt });
      }
      const match =
        /^\/bridge\/sessions\/([a-f0-9]{64})(?:\/(result|poll))?$/.exec(
          req.url || "",
        );
      if (!match) return reply(res, 404, { error: "not_found" });
      const [, id, action] = match;
      const session = sessions.get(id);
      if (!session) return reply(res, 410, { error: "expired" });
      if (req.method === "GET" && !action) {
        if (session.result) return reply(res, 409, { error: "already_sent" });
        return reply(res, 200, {
          receiverPublicKey: session.receiverPublicKey,
          appId: session.appId,
          scopes: session.scopes,
          expectedUserId: session.expectedUserId,
          expiresAt: session.expiresAt,
        });
      }
      if (req.method === "POST" && action === "result") {
        const envelope = await body(req);
        // Recheck after awaiting the upload: another request may have consumed or expired the pair.
        if (sessions.get(id) !== session || session.expiresAt <= now())
          return reply(res, 410, { error: "expired" });
        if (
          !envelope ||
          Object.keys(envelope).sort().join(",") !==
            "ciphertext,nonce,publicKey,version" ||
          envelope.version !== 1 ||
          !publicKey(envelope.publicKey) ||
          !b64(envelope.nonce, 16, 16) ||
          !b64(envelope.ciphertext, 24, 22_000)
        )
          return reply(res, 400, { error: "invalid_envelope" });
        if (session.result)
          return reply(
            res,
            JSON.stringify(session.result) === JSON.stringify(envelope)
              ? 201
              : 409,
            { ok: true },
          );
        session.result = envelope;
        return reply(res, 201, { ok: true });
      }
      if (
        (req.method === "POST" && action === "poll") ||
        (req.method === "DELETE" && !action)
      ) {
        if (!authorized(req, session))
          return reply(res, 403, { error: "forbidden" });
        if (req.method === "DELETE") {
          sessions.delete(id);
          return reply(res, 200, { ok: true });
        }
        if (!session.result) return reply(res, 202, { pending: true });
        const result = session.result;
        sessions.delete(id);
        return reply(res, 200, result);
      }
      return reply(res, 405, { error: "method" });
    } catch {
      if (!res.headersSent) reply(res, 400, { error: "invalid_request" });
      else res.end();
    }
  });
  server.requestTimeout = 10_000;
  server.headersTimeout = 10_000;
  server.maxHeadersCount = 32;
  server.on("close", () => {
    clearInterval(timer);
    sessions.clear();
  });
  return server;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  createBridgeServer().listen(8081, "0.0.0.0");
}
