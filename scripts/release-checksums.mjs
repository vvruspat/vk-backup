import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { readdir, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const [directory, tag] = process.argv.slice(2);
assert.ok(directory && tag, 'Usage: node scripts/release-checksums.mjs DIRECTORY TAG');
const prefix = `vk-backup_${tag.slice(1)}_`;
const files = (await readdir(directory)).filter((file) => file.startsWith(prefix)).sort();
for (const pattern of [
  /^darwin_aarch64\.dmg$/,
  /^darwin_x64\.dmg$/,
  /^windows_x64-setup\.exe$/,
  /^windows_x64\.msi$/,
  /^linux_amd64\.deb$/,
  /^linux_amd64\.AppImage$/,
]) {
  assert.ok(files.some((file) => pattern.test(file.slice(prefix.length))), `Missing installer: ${pattern}`);
}
const lines = [];
for (const file of files) {
  const path = join(directory, file);
  assert.ok((await stat(path)).size > 0, `Empty installer: ${file}`);
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  lines.push(`${hash.digest('hex')}  ${file}`);
}
await writeFile(join(directory, 'SHA256SUMS.txt'), `${lines.join('\n')}\n`);
console.log(`Verified ${files.length} release files and wrote SHA256SUMS.txt`);
