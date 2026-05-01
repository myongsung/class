import { createHash } from 'node:crypto';
import { createReadStream, createWriteStream, copyFileSync, existsSync, mkdirSync } from 'node:fs';
import { access } from 'node:fs/promises';
import { pipeline } from 'node:stream/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');
const modelDir = resolve(repoRoot, 'src-tauri', 'resources', 'models');

const DEFAULT_HYPERCLOVA_MODEL_URL =
  'https://github.com/myongsung/roosycozy-models/releases/download/model_v1/HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf';
const DEFAULT_ROOSY_MODEL_URL =
  'https://github.com/myongsung/roosycozy-models2/releases/download/model/hyperclovax_roosy_Q4_K_M.gguf';

const models = [
  {
    required: true,
    name: 'HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf',
    envPath: 'ROOSYCOZY_MODEL_PATH',
    envUrl: 'ROOSYCOZY_MODEL_URL',
    envSha256: 'ROOSYCOZY_MODEL_SHA256',
    defaultUrl: DEFAULT_HYPERCLOVA_MODEL_URL,
  },
  {
    required: true,
    name: 'hyperclovax_roosy_Q4_K_M.gguf',
    envPath: 'ROOSYCOZY_ROOSY_MODEL_PATH',
    envUrl: 'ROOSYCOZY_ROOSY_MODEL_URL',
    envSha256: 'ROOSYCOZY_ROOSY_MODEL_SHA256',
    defaultUrl: DEFAULT_ROOSY_MODEL_URL,
  },
];

async function sha256(filePath) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(filePath)) {
    hash.update(chunk);
  }
  return hash.digest('hex');
}

async function ensureHash(filePath, expectedSha256) {
  if (!expectedSha256) return;
  const actual = await sha256(filePath);
  if (actual !== expectedSha256) {
    throw new Error(`모델 sha256이 기대값과 달라요. expected=${expectedSha256} actual=${actual}`);
  }
}

async function ensureOneModel(spec) {
  const modelPath = resolve(modelDir, spec.name);
  const sourcePath = process.env[spec.envPath] ? resolve(process.env[spec.envPath]) : '';
  const sourceUrl = process.env[spec.envUrl]?.trim() || spec.defaultUrl || '';
  const expectedSha256 = process.env[spec.envSha256]?.trim().toLowerCase() || '';

  if (existsSync(modelPath)) {
    await ensureHash(modelPath, expectedSha256);
    console.log(`모델이 이미 준비되어 있어요: ${modelPath}`);
    return;
  }

  if (sourcePath) {
    await access(sourcePath);
    copyFileSync(sourcePath, modelPath);
    await ensureHash(modelPath, expectedSha256);
    console.log(`로컬 모델 파일을 복사했어요: ${sourcePath}`);
    return;
  }

  if (sourceUrl) {
    const response = await fetch(sourceUrl);
    if (!response.ok || !response.body) {
      throw new Error(`모델을 내려받지 못했어요. status=${response.status}`);
    }
    await pipeline(response.body, createWriteStream(modelPath));
    await ensureHash(modelPath, expectedSha256);
    console.log(`원격 모델 파일을 내려받았어요: ${sourceUrl}`);
    return;
  }

  if (spec.required) {
    throw new Error(
      [
        '번들 모델 파일이 없어요.',
        `기대 경로: ${modelPath}`,
        `로컬 빌드라면 src-tauri/resources/models 아래에 ${spec.name} 파일을 두거나,`,
        `CI라면 ${spec.envPath} 또는 ${spec.envUrl} 환경변수를 설정해주세요.`,
        `환경변수가 없으면 기본 릴리즈 자산 URL을 사용합니다: ${spec.defaultUrl}`
      ].join('\n')
    );
  }
}

async function main() {
  mkdirSync(modelDir, { recursive: true });
  for (const spec of models) {
    await ensureOneModel(spec);
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
