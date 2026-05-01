import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');
const targetDir = resolve(repoRoot, 'src-tauri', 'target', 'release');
const runtimeRoot = resolve(targetDir, 'runtime', 'roosycozy-windows-runtime');
const runtimeDirCandidates = [
  resolve(repoRoot, 'src-tauri', 'binaries', 'windows-x64'),
  resolve(repoRoot, 'src-tauri', 'binaries')
];
const requiredDlls = [
  'llama.dll',
  'mtmd.dll',
  'msvcp140.dll',
  'vcruntime140.dll',
  'vcruntime140_1.dll',
  'concrt140.dll'
];

function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else out.push(full);
  }
  return out;
}

function resolveWindowsLlamaServerExecutable() {
  const explicitServerPath = resolve(repoRoot, 'src-tauri', 'binaries', 'llama-server.exe');
  if (existsSync(explicitServerPath)) return explicitServerPath;
  for (const runtimeDir of runtimeDirCandidates) {
    if (!existsSync(runtimeDir)) continue;
    const files = walk(runtimeDir);
    const found = files.find((filePath) => {
      const lower = filePath.toLowerCase();
      return lower.endsWith('/llama-server.exe') || lower.endsWith('\\llama-server.exe');
    });
    if (found) return found;
  }
  throw new Error('Windows llama-server 실행 파일을 찾지 못했어요. runtime zip 안에 llama-server.exe가 포함되어야 해요.');
}

function resolveRuntimeDlls() {
  for (const runtimeDir of runtimeDirCandidates) {
    if (!existsSync(runtimeDir)) continue;
    const files = walk(runtimeDir);
    const picked = requiredDlls.map((name) => {
      const found = files.find((filePath) => filePath.toLowerCase().endsWith(`/${name}`) || filePath.toLowerCase().endsWith(`\\${name}`));
      return found ? { name, source: found } : null;
    });
    if (!picked.every(Boolean)) continue;

    const allDlls = files
      .filter((filePath) => filePath.toLowerCase().endsWith('.dll'))
      .map((source) => ({ name: source.split(/[\\/]/).pop(), source }));
    if (allDlls.length) return allDlls;
  }
  throw new Error(`Windows runtime DLL을 모두 찾지 못했어요. 필수 파일: ${requiredDlls.join(', ')}`);
}

function main() {
  const runtimeDlls = resolveRuntimeDlls();
  const resolvedLlamaServerPath = resolveWindowsLlamaServerExecutable();

  rmSync(runtimeRoot, { recursive: true, force: true });
  mkdirSync(runtimeRoot, { recursive: true });

  copyFileSync(resolvedLlamaServerPath, resolve(runtimeRoot, 'llama-server.exe'));
  for (const dll of runtimeDlls) {
    copyFileSync(dll.source, resolve(runtimeRoot, dll.name));
  }

  console.log(`Windows runtime 번들을 준비했어요: ${runtimeRoot}`);
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}
