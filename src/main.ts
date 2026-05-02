import { invoke } from '@tauri-apps/api/core';
import './styles.css';

type BootStatusKind = 'loading' | 'updating' | 'repairing' | 'ready' | 'error';

const BOOT_SPLASH_MIN_VISIBLE_MS = 900;
const BOOT_SPLASH_MAX_VISIBLE_MS = 2200;
const BOOT_SPLASH_HARD_TIMEOUT_MS = 4500;
const WINDOWS_BACKGROUND_UPDATE_DELAY_MS = 18000;
const AUTO_UPDATE_ON_BOOT = false;
const isWindowsDesktop = () => typeof navigator !== 'undefined' && /Windows/i.test(String(navigator.userAgent || ''));
const isDesktopRuntime = () => typeof window !== 'undefined' && !!(window as any).__TAURI_INTERNALS__;

function setBootSplash(message: string, hint = '', kind: BootStatusKind = 'loading') {
  const splash = document.getElementById('boot-splash');
  const status = document.getElementById('boot-splash-status');
  const hintEl = document.getElementById('boot-splash-hint');
  if (!splash || !status || !hintEl) return;
  splash.setAttribute('data-kind', kind);
  status.textContent = message;
  hintEl.textContent = hint || '잠시만 기다려주세요.';
}

function hideBootSplash(delay = 180) {
  const splash = document.getElementById('boot-splash');
  if (!splash) return;
  window.setTimeout(() => {
    splash.classList.add('is-hidden');
  }, delay);
}

let bootSplashReleased = false;

function releaseBootSplash(
  message = '준비가 끝났어요.',
  hint = '지금부터 바로 기록과 AI 대화를 시작할 수 있어요.',
  kind: BootStatusKind = 'ready',
  delay = 180
) {
  if (bootSplashReleased) return;
  bootSplashReleased = true;
  setBootSplash(message, hint, kind);
  hideBootSplash(delay);
}

function showUpdateToast(message: string, autoHide: boolean = false) {
  let toast = document.getElementById('update-toast');

  if (!toast) {
    toast = document.createElement('div');
    toast.id = 'update-toast';
    toast.style.position = 'fixed';
    toast.style.bottom = '20px';
    toast.style.right = '20px';
    toast.style.backgroundColor = 'rgba(15, 23, 42, 0.92)';
    toast.style.color = '#fff';
    toast.style.padding = '14px 18px';
    toast.style.borderRadius = '12px';
    toast.style.boxShadow = '0 12px 32px rgba(15, 23, 42, 0.22)';
    toast.style.zIndex = '9999';
    toast.style.fontFamily = 'sans-serif';
    toast.style.fontSize = '13px';
    toast.style.lineHeight = '1.55';
    toast.style.transition = 'opacity 0.25s ease';
    toast.style.whiteSpace = 'pre-wrap';
    document.body.appendChild(toast);
  }

  toast.innerText = message;
  toast.style.opacity = '1';

  if (autoHide) {
    window.setTimeout(() => {
      if (toast) toast.style.opacity = '0';
    }, 3000);
  }

  return toast;
}

function renderBootFallback(message: string, detail = '') {
  const app = document.getElementById('app');
  if (!app || app.childElementCount > 0) return;
  app.innerHTML = `
    <div style="min-height:100vh;display:flex;align-items:center;justify-content:center;background:#f6f8fb;padding:24px;font-family:Pretendard,-apple-system,BlinkMacSystemFont,system-ui,sans-serif;color:#233247;">
      <div style="width:min(560px,100%);background:rgba(255,255,255,0.96);border:1px solid rgba(115,138,168,0.14);box-shadow:0 24px 60px rgba(52,72,104,0.10);border-radius:24px;padding:28px 28px 24px;">
        <div style="font-size:24px;font-weight:800;letter-spacing:-0.03em;">기본 화면을 여는 중 문제가 생겼어요.</div>
        <div style="margin-top:10px;font-size:14px;line-height:1.7;color:#5f6f86;">${message}</div>
        ${detail ? `<pre style="margin-top:16px;padding:14px 16px;background:#f7f9fc;border-radius:16px;color:#6b7b92;font-size:12px;line-height:1.5;white-space:pre-wrap;word-break:break-word;">${detail.replace(/[&<>]/g, (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[char] as string))}</pre>` : ''}
      </div>
    </div>
  `;
}

function handleBootFailure(label: string, error: unknown) {
  const message = String(error ?? '알 수 없는 오류');
  console.error(label, error);
  releaseBootSplash(
    '문제가 생겼지만 화면은 먼저 열어둘게요.',
    '초기화 오류가 있어도 상태를 확인할 수 있게 기본 화면을 열어두겠습니다.',
    'error',
    80
  );
  renderBootFallback(label, message);
  showUpdateToast(`⚠️ ${label}\n${message}`, false);
}

async function checkAndUpdateApp() {
  try {
    showUpdateToast('🔄 백그라운드에서 새 버전과 Windows 런타임을 확인하고 있어요.', true);
    const updatePromise = invoke<string>('check_and_update');
    const result = await updatePromise;

    if (result.includes('업데이트 완료')) {
      showUpdateToast(
        '✅ 업데이트 파일 준비를 마쳤어요.\n잠시 후 앱이 자동으로 다시 열리며 새 버전이 적용됩니다.'
      );
      window.setTimeout(() => {
        void invoke('exit_for_update').catch((error) => {
          console.error('업데이트 종료 에러:', error);
        });
      }, 900);
      return;
    }

    if (result.includes('복구했어요')) {
      showUpdateToast(`🛠️ ${result}`, false);
      return;
    }

    showUpdateToast('✨ 현재 최신 버전이에요.', true);
  } catch (error) {
    console.error('업데이트 에러:', error);
    const errMsg = String(error ?? '');

    if (errMsg.includes('No asset found for target')) {
      const toast = document.getElementById('update-toast');
      if (toast) toast.style.opacity = '0';
      return;
    }

    showUpdateToast(`❌ 자동 업데이트 확인에 실패했어요.\n${errMsg}`, false);
  }
}

setBootSplash(
  '앱을 준비하고 있어요…',
  '화면은 곧 열리고, 업데이트와 Windows 런타임 점검은 백그라운드에서 이어집니다.',
  'loading'
);
window.setTimeout(() => {
  releaseBootSplash();
}, BOOT_SPLASH_MIN_VISIBLE_MS);

window.setTimeout(() => {
  releaseBootSplash(
    '기본 화면을 먼저 열어둘게요.',
    '업데이트와 Windows 런타임 점검은 백그라운드에서 이어서 진행합니다.',
    'ready',
    120
  );
}, BOOT_SPLASH_MAX_VISIBLE_MS);

window.setTimeout(() => {
  releaseBootSplash(
    '기본 화면을 먼저 열어둘게요.',
    '뒤에서 준비 중인 작업이 남아 있어도 우선 앱을 사용할 수 있게 할게요.',
    'ready',
    60
  );
}, BOOT_SPLASH_HARD_TIMEOUT_MS);

if (AUTO_UPDATE_ON_BOOT && isWindowsDesktop()) {
  window.setTimeout(() => {
    void checkAndUpdateApp();
  }, WINDOWS_BACKGROUND_UPDATE_DELAY_MS);
}

window.addEventListener('error', (event) => {
  handleBootFailure('앱 초기화 중 오류가 발생했어요.', event.error ?? event.message);
});

window.addEventListener('unhandledrejection', (event) => {
  handleBootFailure('처리되지 않은 초기화 오류가 발생했어요.', event.reason);
});

window.setTimeout(() => {
  void import('./main/app')
    .then(({ initApp }) => Promise.resolve(initApp()))
    .catch((error) => {
      handleBootFailure('앱 시작 준비를 마치지 못했어요.', error);
    });
}, 0);
