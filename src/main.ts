import { invoke } from '@tauri-apps/api/core';
import './styles.css';
import { initApp } from './main/app';

type BootStatusKind = 'loading' | 'updating' | 'repairing' | 'ready' | 'error';

const BOOT_SPLASH_MIN_VISIBLE_MS = 900;
const BOOT_SPLASH_MAX_VISIBLE_MS = 2200;

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
initApp();
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
  void checkAndUpdateApp();
}, BOOT_SPLASH_MAX_VISIBLE_MS + 1200);
