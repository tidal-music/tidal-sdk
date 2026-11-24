import { build } from 'esbuild';
import { cp, rm } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';

// A clean dist keeps chunks from earlier builds out of the package.
await rm('dist', { recursive: true, force: true });
execFileSync('tsc', [], { stdio: 'inherit' });
await build({
  entryPoints: ['src/index.ts', 'src/bindings.ts'],
  splitting: true,
  outdir: 'dist',
  bundle: true,
  format: 'esm',
  platform: 'browser',
  target: 'es2022',
  sourcemap: true,
});
await cp('generated/player.wasm', 'dist/player.wasm');
