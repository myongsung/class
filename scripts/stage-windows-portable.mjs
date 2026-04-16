import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');
const targetDir = resolve(repoRoot, 'src-tauri', 'target', 'release');
const portableRoot = resolve(targetDir, 'portable', 'roosycozy-windows-x64');
const supportRoot = resolve(portableRoot, 'RoosyCozy');
const legacyResourcesRoot = resolve(portableRoot, 'resources');
const legacySidecarRoot = resolve(portableRoot, 'sidecar');
const executablePath = resolve(targetDir, 'roosycozy.exe');
const modelSpecs = [];
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
  if (existsSync(sidecarPath)) {
    return sidecarPath;
  }
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

function resolveModelFiles() {
  return modelSpecs.flatMap((spec) => {
    const builtModelPath = resolve(targetDir, 'models', spec.name);
    const sourceModelPath = resolve(repoRoot, 'src-tauri', 'resources', 'models', spec.name);
    const chosen = existsSync(builtModelPath) ? builtModelPath : sourceModelPath;
    if (!existsSync(chosen)) {
      if (spec.required) {
        throw new Error(`필수 모델 파일을 찾지 못했어요: ${spec.name}`);
      }
      return [];
    }
    return [{
      name: spec.name,
      source: chosen,
    }];
  });
}

function main() {
  const modelFiles = resolveModelFiles();
  const runtimeDlls = resolveRuntimeDlls();
  const resolvedSidecarPath = resolveWindowsSidecarExecutable();

  requireFile(executablePath, 'Windows 실행 파일');
  requireFile(resolvedSidecarPath, 'Windows sidecar');

  rmSync(portableRoot, { recursive: true, force: true });
  mkdirSync(resolve(supportRoot, 'resources', 'models'), { recursive: true });
  mkdirSync(resolve(supportRoot, 'sidecar'), { recursive: true });
  mkdirSync(resolve(legacyResourcesRoot, 'models'), { recursive: true });
  mkdirSync(legacySidecarRoot, { recursive: true });

  copyFileSync(executablePath, resolve(portableRoot, 'roosycozy.exe'));
  for (const model of modelFiles) {
    copyFileSync(model.source, resolve(supportRoot, 'resources', 'models', model.name));
    copyFileSync(model.source, resolve(legacyResourcesRoot, 'models', model.name));
  }
  copyFileSync(resolvedSidecarPath, resolve(supportRoot, 'sidecar', 'llama-sidecar-x86_64-pc-windows-msvc.exe'));
  copyFileSync(resolvedSidecarPath, resolve(legacySidecarRoot, 'llama-sidecar-x86_64-pc-windows-msvc.exe'));
  for (const dll of runtimeDlls) {
    copyFileSync(dll.source, resolve(supportRoot, 'sidecar', dll.name));
    copyFileSync(dll.source, resolve(legacySidecarRoot, dll.name));
  }

  console.log(`Windows portable 번들을 준비했어요: ${portableRoot}`);
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}
