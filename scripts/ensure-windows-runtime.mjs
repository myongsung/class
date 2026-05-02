import { copyFileSync, createWriteStream, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { pipeline } from 'node:stream/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');
const runtimeDir = resolve(repoRoot, 'src-tauri', 'binaries', 'windows-x64');
const archivePath = resolve(repoRoot, 'src-tauri', 'binaries', 'windows-x64-runtime.zip');
const binariesDir = resolve(repoRoot, 'src-tauri', 'binaries');
const canonicalServerPath = resolve(binariesDir, 'llama-server.exe');
const runtimeUrl =
  process.env.ROOSYCOZY_WINDOWS_RUNTIME_URL?.trim() ||
  'https://github.com/ggml-org/llama.cpp/releases/download/b8763/llama-b8763-bin-win-cpu-x64.zip';

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
    if (entry.isDirectory()) {
      out.push(...walk(full));
    } else {
      out.push(full);
    }
  }
  return out;
}

function hasRuntimeBundle() {
  if (!existsSync(runtimeDir)) return false;
  const files = walk(runtimeDir).map((filePath) => filePath.toLowerCase());
  const hasServer = residentServerCandidates.some((name) =>
    files.some((filePath) => filePath.endsWith(`/${name}`) || filePath.endsWith(`\\${name}`))
  );
  return hasServer && requiredDlls.every((name) => files.some((filePath) => filePath.endsWith(`/${name}`) || filePath.endsWith(`\\${name}`)));
}

function resolveCandidateFile(rootDir, candidates) {
  const files = walk(rootDir);
  for (const candidate of candidates) {
    const found = files.find((filePath) => {
      const lower = filePath.toLowerCase();
      return lower.endsWith(`/${candidate.toLowerCase()}`) || lower.endsWith(`\\${candidate.toLowerCase()}`);
    });
    if (found) return found;
  }
  return null;
}

function copyWindowsSystemDlls() {
  if (process.platform !== 'win32') return;
  const systemRoot = process.env.SystemRoot || 'C:\\Windows';
  const system32 = resolve(systemRoot, 'System32');
  for (const name of ['msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'concrt140.dll']) {
    const source = resolve(system32, name);
    if (existsSync(source)) {
      copyFileSync(source, resolve(runtimeDir, name));
    }
  }
}

function canonicalizeRuntimeExecutables() {
  mkdirSync(binariesDir, { recursive: true });

  const serverSource = resolveCandidateFile(runtimeDir, residentServerCandidates);
  if (!serverSource) {
    throw new Error(`Windows runtime에서 llama-server 실행 파일을 찾지 못했어요: ${residentServerCandidates.join(', ')}`);
  }
  copyFileSync(serverSource, canonicalServerPath);

  const runtimeDlls = resolveRuntimeDlls();
  for (const dll of runtimeDlls) {
    copyFileSync(dll.source, resolve(runtimeDir, dll.name));
  }
}

function runOrThrow(command, args) {
  const result = spawnSync(command, args, { stdio: 'inherit' });
  if (result.status !== 0) {
    throw new Error(`${command} 실행에 실패했어요. exit=${result.status ?? 'unknown'}`);
  }
}

function extractArchive() {
  if (process.platform === 'win32') {
    runOrThrow('powershell', [
      '-NoProfile',
      '-Command',
      `Expand-Archive -LiteralPath '${archivePath.replace(/'/g, "''")}' -DestinationPath '${runtimeDir.replace(/'/g, "''")}' -Force`
    ]);
    return;
  }
  runOrThrow('unzip', ['-o', archivePath, '-d', runtimeDir]);
}

async function downloadArchive() {
  const response = await fetch(runtimeUrl);
  if (!response.ok || !response.body) {
    throw new Error(`Windows runtime zip을 내려받지 못했어요. status=${response.status} url=${runtimeUrl}`);
  }
  await pipeline(response.body, createWriteStream(archivePath));
}

async function main() {
  if (hasRuntimeBundle()) {
    console.log(`Windows runtime이 이미 준비되어 있어요: ${runtimeDir}`);
    return;
  }

  mkdirSync(runtimeDir, { recursive: true });
  rmSync(runtimeDir, { recursive: true, force: true });
  mkdirSync(runtimeDir, { recursive: true });

  console.log(`Windows runtime zip을 내려받을게요: ${runtimeUrl}`);
  await downloadArchive();
  extractArchive();
  copyWindowsSystemDlls();
  canonicalizeRuntimeExecutables();

  if (!hasRuntimeBundle()) {
    throw new Error(
      [
        'Windows runtime zip 압축을 풀었지만 필요한 DLL 구성이 보이지 않아요.',
        `확인 폴더: ${runtimeDir}`,
        `필수 DLL: ${requiredDlls.join(', ')}`,
        `필수 실행 파일: ${residentServerCandidates.join(' | ')}`
      ].join('\n')
    );
  }

  console.log(`Windows runtime을 준비했어요: ${runtimeDir}`);
  console.log(`Canonical llama-server: ${canonicalServerPath}`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
