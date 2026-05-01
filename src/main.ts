import { invoke } from '@tauri-apps/api/core';
import './styles.css';
import { initApp } from './main/app';

type BootStatusKind = 'loading' | 'updating' | 'repairing' | 'ready' | 'error';

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
  let splashReleased = false;
  let timeoutHandle = 0;
  const releaseSplashForBackgroundWork = (message: string, hint: string) => {
    if (splashReleased) return;
    splashReleased = true;
    setBootSplash(message, hint, 'ready');
    hideBootSplash(220);
  };

  try {
    setBootSplash(
      '업데이트를 마무리하는 중이에요…',
      '절대 화면을 중간에 닫지 마세요. 예상 소요 시간은 3분 내외입니다.',
      'loading'
    );
    showUpdateToast('🔄 새 버전을 확인하고 있습니다...');
    const updatePromise = invoke<string>('check_and_update');
    timeoutHandle = window.setTimeout(() => {
      releaseSplashForBackgroundWork(
        '기본 화면을 먼저 열어둘게요.',
        '업데이트와 Windows 런타임 점검은 백그라운드에서 이어서 진행합니다.'
      );
      showUpdateToast('🕒 백그라운드에서 업데이트와 런타임을 계속 점검하고 있어요.', true);
    }, 4500);

    const result = await updatePromise;
    if (timeoutHandle) {
      window.clearTimeout(timeoutHandle);
    }

    if (result.includes('업데이트 완료')) {
      setBootSplash(
        '업데이트 파일 준비를 마쳤어요.',
        '잠시 후 자동으로 앱이 다시 열리며 새 버전이 적용됩니다.',
        'updating'
      );
      showUpdateToast('✅ 업데이트를 받았어요. 잠시 후 새 버전으로 자동 전환됩니다.');
      window.setTimeout(() => {
        void invoke('exit_for_update').catch((error) => {
          console.error('업데이트 종료 에러:', error);
        });
      }, 900);
      return;
    }

    if (result.includes('복구했어요')) {
      if (!splashReleased) {
        setBootSplash(
          '업데이트 마무리를 마쳤어요.',
          '이제 바로 채팅을 시작할 수 있어요.',
          'repairing'
        );
      }
      showUpdateToast(`🛠️ ${result}`, false);
      if (!splashReleased) {
        hideBootSplash(520);
      }
      return;
    }

    showUpdateToast('✨ 현재 최신 버전입니다.', true);
    if (!splashReleased) {
      setBootSplash(
        '준비가 끝났어요.',
        '지금부터 바로 기록과 AI 대화를 시작할 수 있어요.',
        'ready'
      );
      hideBootSplash(380);
    }
  } catch (error) {
    if (timeoutHandle) {
      window.clearTimeout(timeoutHandle);
    }
    console.error('업데이트 에러:', error);
    const errMsg = String(error ?? '');

    if (errMsg.includes('No asset found for target')) {
      const toast = document.getElementById('update-toast');
      if (toast) toast.style.opacity = '0';
      if (!splashReleased) {
        setBootSplash(
          '준비가 끝났어요.',
          '지금부터 바로 기록과 AI 대화를 시작할 수 있어요.',
          'ready'
        );
        hideBootSplash(280);
      }
      return;
    }

    showUpdateToast(`❌ 자동 업데이트 확인에 실패했어요.\n${errMsg}`, false);
    if (!splashReleased) {
      setBootSplash(
        '업데이트 확인에 실패했어요.',
        '인터넷 상태를 확인한 뒤 다시 열면 자동으로 한 번 더 점검합니다.',
        'error'
      );
      hideBootSplash(1200);
    }
  }
}

setBootSplash(
  '업데이트를 준비하고 있어요…',
  '절대 화면을 중간에 닫지 마세요. 예상 소요 시간은 3분 내외입니다.',
  'loading'
);
initApp();
void checkAndUpdateApp();
