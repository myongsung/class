import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');
const modelName = 'HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf';
const targetDir = resolve(repoRoot, 'src-tauri', 'target', 'release');
const portableRoot = resolve(targetDir, 'portable', 'roosycozy-windows-x64');
const executablePath = resolve(targetDir, 'roosycozy.exe');
const builtModelPath = resolve(targetDir, 'models', modelName);
const sourceModelPath = resolve(repoRoot, 'src-tauri', 'resources', 'models', modelName);
const sidecarPath = resolve(repoRoot, 'src-tauri', 'binaries', 'llama-sidecar-x86_64-pc-windows-msvc.exe');
const runtimeDirCandidates = [
  resolve(repoRoot, 'src-tauri', 'binaries', 'windows-x64'),
  resolve(repoRoot, 'src-tauri', 'binaries')
];
const requiredRuntimeDlls = ['llama.dll', 'mtmd.dll'];
const executableCandidates = [
  'llama-sidecar-x86_64-pc-windows-msvc.exe',
  'llama-cli.exe',
  'llama-server.exe'
];

function requireFile(filePath, label) {
  if (!existsSync(filePath)) {
    throw new Error(`${label} 파일을 찾지 못했어요: ${filePath}`);
  }
}

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

function resolveRuntimeDlls() {
  for (const runtimeDir of runtimeDirCandidates) {
    if (!existsSync(runtimeDir)) continue;
    const files = walk(runtimeDir);
    const dlls = files.filter((filePath) => filePath.toLowerCase().endsWith('.dll'));
    if (!dlls.length) continue;

    const missing = requiredRuntimeDlls.filter(
      (name) => !dlls.some((filePath) => filePath.toLowerCase().endsWith(`/${name}`) || filePath.toLowerCase().endsWith(`\\${name}`))
    );

    if (missing.length) {
      throw new Error(
        [
          `Windows 런타임 DLL이 아직 부족해요: ${missing.join(', ')}`,
          `확인한 폴더: ${runtimeDir}`,
          '같은 빌드에서 나온 llama.cpp Windows DLL 묶음을 여기에 넣어주세요.'
        ].join('\n')
      );
    }

    return dlls.map((source) => ({
      name: source.split(/[\\/]/).pop(),
      source
    }));
  }

  throw new Error(
    [
      'Windows sidecar 런타임 DLL을 찾지 못했어요.',
      `필수 파일: ${requiredRuntimeDlls.join(', ')}`,
      `추천 위치: ${runtimeDirCandidates[0]}`,
      '같은 빌드에서 나온 Windows DLL 묶음을 추가한 뒤 다시 패키징해주세요.'
    ].join('\n')
  );
}

function resolveWindowsSidecarExecutable() {
  for (const runtimeDir of runtimeDirCandidates) {
    if (!existsSync(runtimeDir)) continue;
    const files = walk(runtimeDir);
    for (const candidate of executableCandidates) {
      const found = files.find((filePath) => filePath.toLowerCase().endsWith(`/${candidate}`) || filePath.toLowerCase().endsWith(`\\${candidate}`));
      if (found) return found;
    }
  }
  return sidecarPath;
}

function main() {
  const modelPath = existsSync(builtModelPath) ? builtModelPath : sourceModelPath;
  const runtimeDlls = resolveRuntimeDlls();
  const resolvedSidecarPath = resolveWindowsSidecarExecutable();

  requireFile(executablePath, 'Windows 실행 파일');
  requireFile(modelPath, '모델');
  requireFile(resolvedSidecarPath, 'Windows sidecar');

  rmSync(portableRoot, { recursive: true, force: true });
  mkdirSync(resolve(portableRoot, 'resources', 'models'), { recursive: true });
  mkdirSync(resolve(portableRoot, 'sidecar'), { recursive: true });

  copyFileSync(executablePath, resolve(portableRoot, 'roosycozy.exe'));
  copyFileSync(modelPath, resolve(portableRoot, 'resources', 'models', modelName));
  copyFileSync(resolvedSidecarPath, resolve(portableRoot, 'sidecar', 'llama-sidecar-x86_64-pc-windows-msvc.exe'));
  for (const dll of runtimeDlls) {
    copyFileSync(dll.source, resolve(portableRoot, 'sidecar', dll.name));
  }

  console.log(`Windows portable 번들을 준비했어요: ${portableRoot}`);
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}
