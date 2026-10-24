// Compiles the browser code, web/src/*.ts and the plugins', plugins/*/*.ts: checks the TypeScript, then compiles each script on its own into a plain
// script, wrapped so that its names stay its own: plain scripts, not modules, work in a site opened from disk. cargo
// build runs it, by build.rs, writing the scripts into the folder it is given, where the binary takes them from:
//
//     node web/build.mjs <folder>
//
// The tools it needs, TypeScript and esbuild at the versions package.json pins, are installed under target/web, with
// the rest of what builds make, so nothing but target/ and the folder given is ever written.
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const web = dirname(fileURLToPath(import.meta.url));
const top = join(web, '..');
const sources = join(web, 'src');
const tools = join(top, 'target', 'web');
const out = process.argv[2];
if (!out) throw new Error('usage: node web/build.mjs <folder>');

// The tools, installed again whenever package.json or its lockfile has changed since they were.
mkdirSync(tools, { recursive: true });
const pinned = ['package.json', 'package-lock.json'];
const stale = !existsSync(join(tools, 'node_modules'))
  || pinned.some((f) => !existsSync(join(tools, f)) || readFileSync(join(tools, f), 'utf8') !== readFileSync(join(web, f), 'utf8'));
if (stale) {
  for (const f of pinned) copyFileSync(join(web, f), join(tools, f));
  execFileSync('npm', ['ci', '--silent', '--no-audit', '--no-fund'], { cwd: tools, stdio: 'inherit' });
}

execFileSync(process.execPath, [join(tools, 'node_modules', 'typescript', 'bin', 'tsc'), '--noEmit', '-p', join(web, 'tsconfig.json')], { stdio: 'inherit' });
const esbuild = await import(pathToFileURL(join(tools, 'node_modules', 'esbuild', 'lib', 'main.js')).href);

// The page's scripts are web/src/*.ts but the helpers they share, written as <folder>/*.js; the plugins' are their
// own folders', written as <folder>/plugins/<plugin>/*.js.
const LIBRARIES = ['dom.ts'];
const scripts = (dir) => readdirSync(dir).filter((f) => f.endsWith('.ts') && !f.endsWith('.d.ts') && !LIBRARIES.includes(f))
  .map((f) => join(dir, f));
const plugins = join(top, 'plugins');
const entries = [
  ...scripts(sources),
  ...(existsSync(plugins) ? readdirSync(plugins) : []).flatMap((p) => scripts(join(plugins, p))),
].map((f) => [f, join(out, relative(f.startsWith(sources) ? sources : top, f).replace(/\.ts$/, '.js'))]);
await Promise.all(entries.map(([entry, outfile]) => esbuild.build({
  entryPoints: [entry], outfile, bundle: true, format: 'iife', target: 'es2022', charset: 'utf8', logLevel: 'warning',
})));
