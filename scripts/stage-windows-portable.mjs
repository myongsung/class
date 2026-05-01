import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');
const targetDir = resolve(repoRoot, 'src-tauri', 'target', 'release');
const portableRoot = resolve(targetDir, 'portable', 'roosycozy-windows-x64');
const executablePath = resolve(targetDir, 'roosycozy.exe');
const supportRoot = resolve(portableRoot, 'RoosyCozy');
const sidecarRoot = resolve(supportRoot, 'sidecar');
const resourcesRoot = resolve(supportRoot, 'resources');
const modelSourceRoot = resolve(repoRoot, 'src-tauri', 'resources', 'models');
const canonicalServerPath = resolve(repoRoot, 'src-tauri', 'binaries', 'llama-server.exe');
const runtimeDirCandidates = [
  resolve(repoRoot, 'src-tauri', 'binaries', 'windows-x64'),
  resolve(repoRoot, 'src-tauri', 'binaries')
];
const residentServerCandidates = ['llama-server.exe', 'llama-server'];
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

function resolveCandidateFile(candidates) {
  for (const runtimeDir of runtimeDirCandidates) {
    if (!existsSync(runtimeDir)) continue;
    const files = walk(runtimeDir);
    for (const candidate of candidates) {
      const found = files.find((filePath) => {
        const lower = filePath.toLowerCase();
        return lower.endsWith(`/${candidate.toLowerCase()}`) || lower.endsWith(`\\${candidate.toLowerCase()}`);
      });
      if (found) return found;
    }
  }
  return null;
}

function resolveRuntimeDll(name) {
  return resolveCandidateFile([name]);
}

function main() {
  if (!existsSync(executablePath)) {
    throw new Error(`Windows 실행 파일을 찾지 못했어요: ${executablePath}`);
  }

  rmSync(portableRoot, { recursive: true, force: true });
  mkdirSync(portableRoot, { recursive: true });
  mkdirSync(sidecarRoot, { recursive: true });
  mkdirSync(resourcesRoot, { recursive: true });
  copyFileSync(executablePath, resolve(portableRoot, 'roosycozy.exe'));

  const residentServer = existsSync(canonicalServerPath) ? canonicalServerPath : resolveCandidateFile(residentServerCandidates);
  if (!residentServer) {
    throw new Error('Windows llama-server 실행 파일을 찾지 못했어요. 먼저 runtime bundle을 준비해주세요.');
  }
  copyFileSync(residentServer, resolve(sidecarRoot, 'llama-server.exe'));

  for (const dllName of requiredDlls) {
    const dllSource = resolveRuntimeDll(dllName);
    if (!dllSource) {
      throw new Error(`Windows runtime DLL을 찾지 못했어요: ${dllName}`);
    }
    copyFileSync(dllSource, resolve(sidecarRoot, dllName));
  }

  if (existsSync(modelSourceRoot)) {
    const modelFiles = walk(modelSourceRoot);
    const targetModelsDir = resolve(resourcesRoot, 'models');
    mkdirSync(targetModelsDir, { recursive: true });
    for (const modelFile of modelFiles) {
      const fileName = modelFile.split(/[\\/]/).pop();
      if (!fileName) continue;
      copyFileSync(modelFile, resolve(targetModelsDir, fileName));
    }
  }

  console.log(`Windows portable 번들을 준비했어요: ${portableRoot}`);
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}
