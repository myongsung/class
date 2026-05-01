import { copyFileSync, existsSync, mkdirSync, rmSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');
const targetDir = resolve(repoRoot, 'src-tauri', 'target', 'release');
const portableRoot = resolve(targetDir, 'portable', 'roosycozy-windows-x64');
const executablePath = resolve(targetDir, 'roosycozy.exe');

function main() {
  if (!existsSync(executablePath)) {
    throw new Error(`Windows 실행 파일을 찾지 못했어요: ${executablePath}`);
  }

  rmSync(portableRoot, { recursive: true, force: true });
  mkdirSync(portableRoot, { recursive: true });
  copyFileSync(executablePath, resolve(portableRoot, 'roosycozy.exe'));

  console.log(`Windows portable 번들을 준비했어요: ${portableRoot}`);
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}
