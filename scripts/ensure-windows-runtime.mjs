import { createWriteStream, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { pipeline } from 'node:stream/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');
const runtimeDir = resolve(repoRoot, 'src-tauri', 'binaries', 'windows-x64');
const archivePath = resolve(repoRoot, 'src-tauri', 'binaries', 'windows-x64-runtime.zip');
const runtimeUrl =
  process.env.ROOSYCOZY_WINDOWS_RUNTIME_URL?.trim() ||
  'https://github.com/ggml-org/llama.cpp/releases/download/b8763/llama-b8763-bin-win-cpu-x64.zip';

const requiredDlls = ['llama.dll', 'mtmd.dll'];
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
  return requiredDlls.every((name) => files.some((filePath) => filePath.endsWith(`/${name}`) || filePath.endsWith(`\\${name}`)));
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

  if (!hasRuntimeBundle()) {
    throw new Error(
      [
        'Windows runtime zip 압축을 풀었지만 필요한 DLL 구성이 보이지 않아요.',
        `확인 폴더: ${runtimeDir}`,
        `필수 DLL: ${requiredDlls.join(', ')}`
      ].join('\n')
    );
  }

  console.log(`Windows runtime을 준비했어요: ${runtimeDir}`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
