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
  try {
    setBootSplash(
      '최신 버전과 AI 런타임 상태를 확인하는 중이에요…',
      '첫 실행이거나 복구가 필요한 경우에는 1~2분 정도 더 걸릴 수 있어요.',
      'loading'
    );
    showUpdateToast('🔄 새 버전을 확인하고 있습니다...');
    const result = await invoke<string>('check_and_update');

    if (result.includes('업데이트 완료')) {
      setBootSplash(
        '업데이트 파일을 준비했어요.',
        '앱을 다시 시작하면 새 버전과 AI 런타임이 함께 적용됩니다.',
        'updating'
      );
      showUpdateToast('✅ 업데이트를 받았어요. 적용을 위해 앱을 다시 시작해주세요.');
      window.setTimeout(() => {
        alert('새 버전 다운로드와 교체가 완료되었습니다.\n앱을 다시 실행하면 최신 버전이 적용됩니다.');
      }, 500);
      return;
    }

    if (result.includes('복구했어요')) {
      setBootSplash(
        'AI 런타임 복구를 마쳤어요.',
        '이제 바로 채팅을 시작할 수 있어요.',
        'repairing'
      );
      showUpdateToast(`🛠️ ${result}`, false);
      hideBootSplash(520);
      return;
    }

    showUpdateToast('✨ 현재 최신 버전입니다.', true);
    setBootSplash(
      '준비가 끝났어요.',
      '지금부터 바로 기록과 AI 대화를 시작할 수 있어요.',
      'ready'
    );
    hideBootSplash(380);
  } catch (error) {
    console.error('업데이트 에러:', error);
    const errMsg = String(error ?? '');

    if (errMsg.includes('No asset found for target')) {
      const toast = document.getElementById('update-toast');
      if (toast) toast.style.opacity = '0';
      setBootSplash(
        '준비가 끝났어요.',
        '지금부터 바로 기록과 AI 대화를 시작할 수 있어요.',
        'ready'
      );
      hideBootSplash(280);
      return;
    }

    showUpdateToast(`❌ 자동 업데이트 확인에 실패했어요.\n${errMsg}`, false);
    setBootSplash(
      '업데이트 확인에 실패했어요.',
      '인터넷 상태를 확인한 뒤 다시 열면 자동으로 한 번 더 점검합니다.',
      'error'
    );
    hideBootSplash(1200);
  }
}

initApp();
setBootSplash(
  '프로그램 화면을 여는 중이에요…',
  '기본 화면을 먼저 준비하고, 이어서 업데이트와 AI 런타임을 확인할게요.',
  'loading'
);
void checkAndUpdateApp();
