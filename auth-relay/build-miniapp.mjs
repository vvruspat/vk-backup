import { build } from "esbuild";
await build({ entryPoints: ["auth-relay/miniapp-entry.js"], outfile: "auth-relay/public/miniapp.js", bundle: true, format: "esm", minify: true, target: ["chrome100", "safari16"], legalComments: "eof" });
