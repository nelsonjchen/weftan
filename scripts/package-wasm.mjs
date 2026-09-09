// SPDX-License-Identifier: MPL-2.0
import { copyFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

const root = fileURLToPath(new URL('../', import.meta.url));
const pkgDir = join(root, 'crates/weftan-wasm/pkg');
execFileSync(process.execPath, ['scripts/sync-licenses.mjs', '--check'], { cwd: root, stdio: 'inherit' });
const names = ['LICENSE', 'THIRD_PARTY_NOTICES.md', 'DEPENDENCY_NOTICES.md', 'SOURCE.md'];
for (const name of names) copyFileSync(join(root, name), join(pkgDir, name));
const revision = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
const dirty = execFileSync('git', ['status', '--porcelain', '--untracked-files=no'], { cwd: root, encoding: 'utf8' }).trim();
writeFileSync(join(pkgDir, 'SOURCE-REVISION.txt'), `https://github.com/nelsonjchen/weftan/tree/${revision}\n${dirty ? 'Local modifications present; retain and provide the modified source.\n' : ''}`);
names.push('SOURCE-REVISION.txt');
const manifestPath = join(pkgDir, 'package.json');
if (existsSync(manifestPath)) {
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
  manifest.license = 'MPL-2.0';
  // wasm-pack uses an allowlist; copied notices must be explicitly included.
  if (manifest.files) manifest.files = [...new Set([...manifest.files, ...names])];
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
}
console.log('WASM package includes licenses, attribution, and source instructions.');
