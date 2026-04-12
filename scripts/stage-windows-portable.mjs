import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
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

function requireFile(filePath, label) {
  if (!existsSync(filePath)) {
    throw new Error(`${label} 파일을 찾지 못했어요: ${filePath}`);
  }
}

function resolveRuntimeDlls() {
  for (const runtimeDir of runtimeDirCandidates) {
    if (!existsSync(runtimeDir)) continue;
    const dllNames = readdirSync(runtimeDir).filter((name) => name.toLowerCase().endsWith('.dll'));
    if (!dllNames.length) continue;

    const missing = requiredRuntimeDlls.filter(
      (name) => !dllNames.some((dllName) => dllName.toLowerCase() === name)
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

    return dllNames.map((name) => ({
      name,
      source: resolve(runtimeDir, name)
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

function main() {
  const modelPath = existsSync(builtModelPath) ? builtModelPath : sourceModelPath;
  const runtimeDlls = resolveRuntimeDlls();

  requireFile(executablePath, 'Windows 실행 파일');
  requireFile(modelPath, '모델');
  requireFile(sidecarPath, 'Windows sidecar');

  rmSync(portableRoot, { recursive: true, force: true });
  mkdirSync(resolve(portableRoot, 'resources', 'models'), { recursive: true });
  mkdirSync(resolve(portableRoot, 'sidecar'), { recursive: true });

  copyFileSync(executablePath, resolve(portableRoot, 'roosycozy.exe'));
  copyFileSync(modelPath, resolve(portableRoot, 'resources', 'models', modelName));
  copyFileSync(sidecarPath, resolve(portableRoot, 'sidecar', 'llama-sidecar-x86_64-pc-windows-msvc.exe'));
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
