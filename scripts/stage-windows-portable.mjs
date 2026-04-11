import { copyFileSync, existsSync, mkdirSync, rmSync } from 'node:fs';
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

function requireFile(filePath, label) {
  if (!existsSync(filePath)) {
    throw new Error(`${label} 파일을 찾지 못했어요: ${filePath}`);
  }
}

function main() {
  const modelPath = existsSync(builtModelPath) ? builtModelPath : sourceModelPath;

  requireFile(executablePath, 'Windows 실행 파일');
  requireFile(modelPath, '모델');
  requireFile(sidecarPath, 'Windows sidecar');

  rmSync(portableRoot, { recursive: true, force: true });
  mkdirSync(resolve(portableRoot, 'resources', 'models'), { recursive: true });
  mkdirSync(resolve(portableRoot, 'sidecar'), { recursive: true });

  copyFileSync(executablePath, resolve(portableRoot, 'roosycozy.exe'));
  copyFileSync(modelPath, resolve(portableRoot, 'resources', 'models', modelName));
  copyFileSync(sidecarPath, resolve(portableRoot, 'sidecar', 'llama-sidecar-x86_64-pc-windows-msvc.exe'));

  console.log(`Windows portable 번들을 준비했어요: ${portableRoot}`);
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}
