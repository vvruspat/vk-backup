import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const tag = process.argv[2];
assert.match(tag ?? '', /^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/, 'Use a version tag such as v0.1.0');
const version = tag.slice(1);
const packageJson = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8'));
const tauri = JSON.parse(await readFile(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
const cargo = await readFile(new URL('../src-tauri/Cargo.toml', import.meta.url), 'utf8');
const cargoVersion = cargo.split(/\n\[/)[0].match(/^version\s*=\s*"([^"]+)"/m)?.[1];
for (const [file, actual] of [['package.json', packageJson.version], ['tauri.conf.json', tauri.version], ['Cargo.toml', cargoVersion]]) {
  assert.equal(actual, version, `${file} must match ${tag}`);
}
console.log(`Release version verified: ${tag}`);
