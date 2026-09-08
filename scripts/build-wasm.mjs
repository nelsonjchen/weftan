// SPDX-License-Identifier: MPL-2.0
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const cwd = fileURLToPath(new URL('../', import.meta.url));
execFileSync('wasm-pack', ['build', 'crates/weftan-wasm', ...process.argv.slice(2)], { cwd, stdio: 'inherit' });
execFileSync(process.execPath, ['scripts/package-wasm.mjs'], { cwd, stdio: 'inherit' });
