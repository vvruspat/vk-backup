export const PROTOCOL = "vk-backup-bridge-v1";
export const encode = (bytes) =>
  btoa(String.fromCharCode(...new Uint8Array(bytes)))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");
export const decode = (s) =>
  Uint8Array.from(atob(s.replaceAll("-", "+").replaceAll("_", "/")), (c) =>
    c.charCodeAt(0),
  );
export const validPairId = (value) =>
  typeof value === "string" && /^[a-f0-9]{64}$/.test(value);

export async function encryptResult(receiverPublicKey, pairId, result) {
  if (!validPairId(pairId)) throw new Error("invalid_pair");
  const cryptoApi = globalThis.crypto;
  const ephemeral = await cryptoApi.subtle.generateKey(
    { name: "ECDH", namedCurve: "P-256" },
    false,
    ["deriveBits"],
  );
  const receiver = await cryptoApi.subtle.importKey(
    "raw",
    decode(receiverPublicKey),
    { name: "ECDH", namedCurve: "P-256" },
    false,
    [],
  );
  const shared = await cryptoApi.subtle.deriveBits(
    { name: "ECDH", public: receiver },
    ephemeral.privateKey,
    256,
  );
  const material = await cryptoApi.subtle.importKey(
    "raw",
    shared,
    "HKDF",
    false,
    ["deriveKey"],
  );
  const text = new TextEncoder();
  const key = await cryptoApi.subtle.deriveKey(
    {
      name: "HKDF",
      hash: "SHA-256",
      salt: text.encode(pairId),
      info: text.encode(PROTOCOL),
    },
    material,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt"],
  );
  const nonce = cryptoApi.getRandomValues(new Uint8Array(12));
  const ciphertext = await cryptoApi.subtle.encrypt(
    {
      name: "AES-GCM",
      iv: nonce,
      additionalData: text.encode(pairId),
      tagLength: 128,
    },
    key,
    text.encode(JSON.stringify(result)),
  );
  return {
    version: 1,
    publicKey: encode(
      await cryptoApi.subtle.exportKey("raw", ephemeral.publicKey),
    ),
    nonce: encode(nonce),
    ciphertext: encode(ciphertext),
  };
}
