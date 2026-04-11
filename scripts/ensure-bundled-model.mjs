import { createHash } from 'node:crypto';
import { createReadStream, createWriteStream, copyFileSync, existsSync, mkdirSync } from 'node:fs';
import { access } from 'node:fs/promises';
import { pipeline } from 'node:stream/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');
const modelName = 'HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf';
const modelDir = resolve(repoRoot, 'src-tauri', 'resources', 'models');
const modelPath = resolve(modelDir, modelName);
const sourcePath = process.env.ROOSYCOZY_MODEL_PATH ? resolve(process.env.ROOSYCOZY_MODEL_PATH) : '';
const sourceUrl = process.env.ROOSYCOZY_MODEL_URL?.trim() || '';
const expectedSha256 = process.env.ROOSYCOZY_MODEL_SHA256?.trim().toLowerCase() || '';

async function sha256(filePath) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(filePath)) {
    hash.update(chunk);
  }
  return hash.digest('hex');
}

async function ensureHash(filePath) {
  if (!expectedSha256) return;
  const actual = await sha256(filePath);
  if (actual !== expectedSha256) {
    throw new Error(`모델 sha256이 기대값과 달라요. expected=${expectedSha256} actual=${actual}`);
  }
}

async function main() {
  mkdirSync(modelDir, { recursive: true });

  if (existsSync(modelPath)) {
    await ensureHash(modelPath);
    console.log(`모델이 이미 준비되어 있어요: ${modelPath}`);
    return;
  }

  if (sourcePath) {
    await access(sourcePath);
    copyFileSync(sourcePath, modelPath);
    await ensureHash(modelPath);
    console.log(`로컬 모델 파일을 복사했어요: ${sourcePath}`);
    return;
  }

  if (sourceUrl) {
    const response = await fetch(sourceUrl);
    if (!response.ok || !response.body) {
      throw new Error(`모델을 내려받지 못했어요. status=${response.status}`);
    }
    await pipeline(response.body, createWriteStream(modelPath));
    await ensureHash(modelPath);
    console.log(`원격 모델 파일을 내려받았어요: ${sourceUrl}`);
    return;
  }

  throw new Error(
    [
      '번들 모델 파일이 없어요.',
      `기대 경로: ${modelPath}`,
      '로컬 빌드라면 src-tauri/resources/models 아래에 모델을 두거나,',
      'CI라면 ROOSYCOZY_MODEL_PATH 또는 ROOSYCOZY_MODEL_URL 환경변수를 설정해주세요.'
    ].join('\n')
  );
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
