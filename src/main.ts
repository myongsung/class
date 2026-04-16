import { invoke } from '@tauri-apps/api/core';
import './styles.css';
import { initApp } from './main/app';

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
    showUpdateToast('🔄 새 버전을 확인하고 있습니다...');
    const result = await invoke<string>('check_and_update');

    if (result.includes('업데이트 완료')) {
      showUpdateToast('✅ 업데이트를 받았어요. 적용을 위해 앱을 다시 시작해주세요.');
      window.setTimeout(() => {
        alert('새 버전 다운로드와 교체가 완료되었습니다.\n앱을 다시 실행하면 최신 버전이 적용됩니다.');
      }, 500);
      return;
    }

    if (result.includes('복구했어요')) {
      showUpdateToast(`🛠️ ${result}`, false);
      return;
    }

    showUpdateToast('✨ 현재 최신 버전입니다.', true);
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

initApp();
checkAndUpdateApp();
