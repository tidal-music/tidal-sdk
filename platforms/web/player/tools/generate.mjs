// Generates the player crate's TypeScript/Wasm bindings into generated/.
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../../../..');
const run = (command, args, options) =>
  execFileSync(command, args, { stdio: 'inherit', ...options });
const output = (command, args, options) =>
  execFileSync(command, args, { encoding: 'utf8', ...options }).trim();

if (
  !output('rustup', ['target', 'list', '--installed'])
    .split('\n')
    .includes('wasm32-unknown-unknown')
)
  run('rustup', ['target', 'add', 'wasm32-unknown-unknown']);

// ubrn requires the wasm-bindgen CLI to match the crate's wasm-bindgen exactly.
const wanted = readFileSync(join(root, 'Cargo.lock'), 'utf8').match(
  /name = "wasm-bindgen"\nversion = "([^"]+)"/,
)?.[1];
if (!wanted) throw new Error('Cargo.lock has no wasm-bindgen');
const { target_directory } = JSON.parse(
  output('cargo', ['metadata', '--format-version', '1', '--no-deps'], {
    cwd: root,
  }),
);
const tools = join(target_directory, 'player-bindings/tools');
const wasmBindgen = join(tools, 'bin/wasm-bindgen');
if (
  !existsSync(wasmBindgen) ||
  output(wasmBindgen, ['--version']) !== `wasm-bindgen ${wanted}`
)
  run('cargo', [
    'install',
    '--locked',
    '--root',
    tools,
    'wasm-bindgen-cli',
    '--version',
    wanted,
  ]);

run('ubrn', ['build', 'wasm2', '--release', '--config', 'ubrn.config.yaml'], {
  env: { ...process.env, UBRN_WASM_BINDGEN: wasmBindgen },
});
