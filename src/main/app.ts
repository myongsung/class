import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { uid, nowISO, toLocalInputValue, fromLocalInputValue, safeParseJSON, defaultState, normalizeState, loadState, saveState, wipeAll, STATUSES, ensureRecordV8, sealNewRecord, amendSignedRecord, verifyRecordIntegrity, buildSignedBackupEnvelope, verifyBackupEnvelope, reverifyStateRecords, refreshDeviceSignerInfo, trunc } from '../utils';
import type { ActorRef, PlaceType, StoreType, Sensitivity, StepItem, CaseItem, RecordItem } from '../engine';
import { OTHER, casesContainingRecord, addActorToList, buildRecordFromDraft, createCaseWithAdvisors, regenerateCaseAdvisors, buildCaseTimeline, getCaseUpdateCandidates, addRecordsToCase, recordsForCase, recordMainActors } from '../engine';
import { S, setState, ui, toast, runToastAction, log, openConfirm, closeConfirm, openRecordModal, closeRecordModal,  openCaseCreateModal, closeCaseCreateModal, openTimelineModal, closeTimelineModal, openPaperModal, closePaperModal, openPaperPickModal, closePaperPickModal, openCaseUpdateModal, closeCaseUpdateModal, draftRecord, draftRecordEdit, draftCase, draftStep, actorTypeTextFromInternal, actorTypeInternalFromText, getSelectedCase, logs, actorShort, LVS, PLACE_TYPES, STORE_TYPES, UI_OTHER_ACTOR_LABEL, UI_CLASS_ACTOR_LABEL, normalizeActorTypeTextUI, loadRecordEditDraft, resetRecordEditDraft, hasScreenPin, readScreenPin, saveScreenPin, clearScreenPin, normalizeScreenPin, isValidScreenPin, cloneRelationshipGroups, getRelationshipGroups, makeRelationshipActorRef, parseActorChoice } from './state';
import { ensurePaperStyles, buildPaperPayload, computeCasePaperHash } from './paper';
import { render as renderView } from './views';

/* ---------- micro helpers ---------- */
const dlg = (id: string) => document.getElementById(id) as (HTMLDialogElement | HTMLElement | null);
const isDialogEl = (el: unknown): el is HTMLDialogElement => !!el && typeof (el as HTMLDialogElement).showModal === 'function' && typeof (el as HTMLDialogElement).close === 'function';
const closeDlg = (id: string) => { const d = dlg(id); if (isDialogEl(d) && d.open) d.close(); };
const openDlg = (id: string) => { const d = dlg(id); if (isDialogEl(d) && !d.open) d.showModal(); };
const setText = (id: string, text: string) => { const el = document.getElementById(id); if (el) el.textContent = text; };

const SIGNATURE_MODAL_ID = 'signatureModal';
const SIGN_SUCCESS_MODAL_ID = 'signSuccessModal';
const SCREEN_PIN_MODAL_ID = 'screenPinModal';
let _boundWindowDrag = false;
const hasTauriWindow = () => typeof window !== 'undefined' && !!(window as any).__TAURI_INTERNALS__;
const currentDesktopWindow = () => {
  if (!hasTauriWindow()) return null;
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
};

type StrategyModelStatus = {
  windowsDownloadMode: boolean,
  downloadSupported: boolean,
  allReady: boolean,
  storageDir: string,
  models: Array<{
    id: string,
    label: string,
    filename: string,
    available: boolean,
    path: string,
  }>,
};

type StrategyModelDownloadProgress = {
  stage: string,
  modelId: string,
  label: string,
  message: string,
  completed: number,
  total: number,
  downloadedBytes: number,
  totalBytes: number,
  percent: number,
  indeterminate: boolean,
};

const STRATEGY_SYNTHETIC_CACHE_STORAGE_KEY = 'roosycozy_strategy_synthetic_cache_enabled_v1';
const STRATEGY_PERF_LAB_STORAGE_KEY = 'roosycozy_strategy_perf_lab_v1';
const STRATEGY_BACKEND_SETTINGS_STORAGE_KEY = 'roosycozy_strategy_backend_settings_v1';

type StrategyBackendMode = 'llama-server';
type StrategyLlamaServerConfigDraft = {
  hyperclovaUrl: string;
  roosyUrl: string;
  cachePrompt: boolean;
  hyperclovaSlot: string;
  roosySlot: string;
  startupTimeoutMs: string;
  requestTimeoutMs: string;
};

const defaultStrategyLlamaServerConfig = (): StrategyLlamaServerConfigDraft => ({
  hyperclovaUrl: 'http://127.0.0.1:18081/completion',
  roosyUrl: 'http://127.0.0.1:18082/completion',
  cachePrompt: true,
  hyperclovaSlot: '0',
  roosySlot: '0',
  startupTimeoutMs: '90000',
  requestTimeoutMs: '240000',
});

const readStrategyBackendSettings = (): { mode: StrategyBackendMode; llamaServer: StrategyLlamaServerConfigDraft } => {
  const defaults = defaultStrategyLlamaServerConfig();
  const normalizeStoredLoopback = (hyperclovaUrl: string, roosyUrl: string) => {
    const bothLoopback = /^https?:\/\/(127\.0\.0\.1|localhost):\d+\/completion$/i.test(hyperclovaUrl)
      && /^https?:\/\/(127\.0\.0\.1|localhost):\d+\/completion$/i.test(roosyUrl);
    if (bothLoopback && hyperclovaUrl.localeCompare(roosyUrl, undefined, { sensitivity: 'accent' }) === 0) {
      return {
        hyperclovaUrl: defaults.hyperclovaUrl,
        roosyUrl: defaults.roosyUrl,
      };
    }
    return { hyperclovaUrl, roosyUrl };
  };
  try {
    const raw = window.localStorage.getItem(STRATEGY_BACKEND_SETTINGS_STORAGE_KEY);
    if (!raw) return { mode: 'llama-server', llamaServer: defaults };
    const parsed = JSON.parse(raw) as Partial<{ mode: string; llamaServer: Partial<StrategyLlamaServerConfigDraft> }>;
    const merged = {
      ...defaults,
      ...(parsed?.llamaServer || {}),
      cachePrompt: parsed?.llamaServer?.cachePrompt !== false,
    };
    const normalizedEndpoints = normalizeStoredLoopback(
      String(merged.hyperclovaUrl || '').trim() || defaults.hyperclovaUrl,
      String(merged.roosyUrl || '').trim() || defaults.roosyUrl,
    );
    return {
      mode: 'llama-server',
      llamaServer: {
        ...merged,
        ...normalizedEndpoints,
        startupTimeoutMs: String(Math.max(90000, Number(String(merged.startupTimeoutMs || defaults.startupTimeoutMs).trim()) || 90000)),
        requestTimeoutMs: String(Math.max(240000, Number(String(merged.requestTimeoutMs || defaults.requestTimeoutMs).trim()) || 240000)),
      },
    };
  } catch {
    return { mode: 'llama-server', llamaServer: defaults };
  }
};

const persistStrategyBackendSettings = (settings: { mode: StrategyBackendMode; llamaServer: StrategyLlamaServerConfigDraft }) => {
  try {
    window.localStorage.setItem(STRATEGY_BACKEND_SETTINGS_STORAGE_KEY, JSON.stringify({
      mode: 'llama-server',
      llamaServer: settings.llamaServer,
    }));
  } catch {}
};

const readStrategySyntheticCacheSetting = () => {
  return true;
};

const persistStrategySyntheticCacheSetting = (enabled: boolean) => {
  try {
    window.localStorage.setItem(STRATEGY_SYNTHETIC_CACHE_STORAGE_KEY, '1');
  } catch {}
};

const readStrategyPerfLabState = (): StrategyPerfLabState => {
  try {
    const raw = window.localStorage.getItem(STRATEGY_PERF_LAB_STORAGE_KEY);
    if (!raw) return emptyStrategyPerfLabState();
    const parsed = JSON.parse(raw) as Partial<StrategyPerfLabState>;
    return {
      baseline: parsed?.baseline || null,
      drace: parsed?.drace || null,
      baselineRuns: Array.isArray(parsed?.baselineRuns) ? parsed!.baselineRuns! : [],
      draceRuns: Array.isArray(parsed?.draceRuns) ? parsed!.draceRuns! : [],
      draceWarmup: parsed?.draceWarmup || null,
      latest: parsed?.latest || null,
      comparisonWarning: String(parsed?.comparisonWarning || ''),
    };
  } catch {
    return emptyStrategyPerfLabState();
  }
};

const persistStrategyPerfLabState = (state: StrategyPerfLabState) => {
  try {
    window.localStorage.setItem(STRATEGY_PERF_LAB_STORAGE_KEY, JSON.stringify(state));
  } catch {}
};

const resetStrategyPerfLabState = () => {
  const next = emptyStrategyPerfLabState();
  (ui as any).strategyPerfLab = next;
  (ui as any).strategyPerfRunState = emptyStrategyPerfRunState();
  persistStrategyPerfLabState(next);
};

type StrategyMentionSuggestion = {
  groupId: string,
  memberId: string,
  name: string,
  groupLabel: string,
};

const isWindowsDesktop = () => typeof navigator !== 'undefined' && /Windows/i.test(String(navigator.userAgent || ''));

(ui as any).strategySyntheticCacheEnabled = readStrategySyntheticCacheSetting();
(ui as any).strategyPerfLab = readStrategyPerfLabState();
(ui as any).strategyPerfRunState = {
  status: 'idle',
  requestedBackend: 'llama-server',
  requestedMode: 'FullDRACE',
  actualBackend: '',
  startedAt: '',
  finishedAt: '',
  error: '',
  cacheApplied: false,
} as StrategyPerfRunState;
{
  const backendSettings = readStrategyBackendSettings();
  (ui as any).strategyBackendMode = backendSettings.mode;
  (ui as any).strategyLlamaServerConfig = backendSettings.llamaServer;
}

const resetScreenPinModalDraft = () => {
  ui.pinEntryDraft = '';
  ui.pinConfirmDraft = '';
};
const resetScreenPinSettingsDraft = () => {
  ui.pinSettingsDraft = '';
  ui.pinSettingsConfirmDraft = '';
};
const focusScreenPinInput = () => {
  window.setTimeout(() => {
    (document.getElementById('screenPinInput') as HTMLInputElement | null)?.focus();
  }, 0);
};

const focusStrategyChatComposer = () => {
  window.setTimeout(() => {
    const input = document.querySelector('.strategyComposerTextareaOnly') as HTMLTextAreaElement | null;
    input?.focus();
    if (input) {
      const caret = Number((ui as any).strategyChatCaret);
      const nextCaret = Number.isFinite(caret) && caret >= 0 ? Math.min(caret, input.value.length) : input.value.length;
      input.setSelectionRange(nextCaret, nextCaret);
      syncStrategyChatComposerMirror(input);
    }
    (ui as any).strategyChatCaret = -1;
  }, 0);
};

const focusStrategyActionComposer = () => {
  window.setTimeout(() => {
    const input = document.querySelector('.strategyActionPromptInput') as HTMLInputElement | null;
    input?.focus();
    input?.select();
  }, 0);
};

const syncStrategyChatComposerMirror = (el: HTMLTextAreaElement | null) => {
  if (!el) return;
  const wrap = el.closest('.strategyComposerTextareaWrapOnly') as HTMLElement | null;
  const mirror = wrap?.querySelector('.strategyComposerMirror') as HTMLElement | null;
  if (!mirror) return;
  mirror.scrollTop = el.scrollTop;
  mirror.scrollLeft = el.scrollLeft;
};

queueMicrotask(() => {
  if (!hasTauriWindow() || !isWindowsDesktop()) return;
  void refreshStrategyModelStatus({ silent: true }).then(() => render()).catch((err) => {
    log('initial strategy model status failed', err);
  });
});


const closeSignatureModal = () => {
  ui.signatureModalMode = null;
  closeDlg(SIGNATURE_MODAL_ID);
};
const openSignatureModal = (mode: 'create' | 'amend') => {
  ui.signatureModalMode = mode;
  render();
};
const openSignSuccessModal = (message: string, sub: string) => {
  setText('signSuccessMsg', message);
  setText('signSuccessSub', sub);
  openDlg(SIGN_SUCCESS_MODAL_ID);
  window.setTimeout(() => closeDlg(SIGN_SUCCESS_MODAL_ID), 1900);
};

// render()가 전체 DOM을 갈아엎기 때문에(dialog 포함) 리렌더링 중 close 이벤트로 상태가 날아가는 걸 막고,
// 렌더 후 열려있어야 하는 dialog는 다시 열어준다.
let _isRerendering = false;
const bindWindowDragRegionFallback = () => {
  if (_boundWindowDrag || typeof document === 'undefined') return;
  _boundWindowDrag = true;
  document.addEventListener('mousedown', async (e) => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement | null;
    if (!target) return;
    const topbar = target.closest<HTMLElement>('.topbar');
    if (!topbar) return;
    if (target.closest('button, a, input, textarea, select, label, dialog, [role="button"]')) return;
    if (target.closest('.windowControls')) return;
    const win = currentDesktopWindow();
    if (!win) return;
    try {
      await win.startDragging();
    } catch (err) {
      log('window drag start failed', err);
    }
  });
};
const syncDialogs = () => {
  if (ui.viewRecordId) openRecordModal();
  if (ui.caseCreateOpen) openCaseCreateModal();
  if (ui.recordComposerOpen) openDlg('recordComposerModal');
  if (ui.viewTimelineItem) openTimelineModal();
  if (ui.paperPickOpen) openPaperPickModal();
  if (ui.paperCaseId || ui.paperHash) openPaperModal();
  if (ui.updateCaseId) openCaseUpdateModal();
  if (ui.settingsOpen) openDlg('settingsModal');
  if ((ui as any).strategyInputGuideOpen) openDlg('strategyInputGuideModal');
  if (ui.updatesNoteOpen) openDlg('updateNotesModal');
  if (ui.classRosterOpen) openDlg('classRosterModal');
  if ((ui as any).simulationPickerOpen) openDlg('simulationPickerModal');
  if (ui.signatureModalMode) openDlg(SIGNATURE_MODAL_ID);
  if (ui.pinModalOpen) openDlg(SCREEN_PIN_MODAL_ID);
};



const ensureContentProofDraft = () => {
  const draft = (ui as any).contentProofDraft || ((ui as any).contentProofDraft = {
    senderName: '',
    senderAddress: '',
    recipientName: '',
    recipientAddress: '',
  });
  draft.senderName = String(draft.senderName || '');
  draft.senderAddress = String(draft.senderAddress || '');
  draft.recipientName = String(draft.recipientName || '');
  draft.recipientAddress = String(draft.recipientAddress || '');
  return draft;
};



type SimulationPreset = 'shield' | 'balanced' | 'assertive';

type SimulationDraft = {
  evidenceFilter: string;
  scenarioPreset: SimulationPreset;
  parentTone: number;
  escalation: number;
  persistence: number;
  adminSupport: number;
  publicSpread: number;
  legalLeverage: number;
  goal: 'stabilize' | 'document' | 'escalate';
};

type SimulationResult = {
  calculatedAt: string;
  bundleLabel: string;
  selectedCount: number;
  totalAvailable: number;
  responseIndex: number;
  evidencePower: number;
  counterLogic: number;
  escalationRisk: number;
  communicationControl: number;
  recommendedTone: string;
  recommendedAction: string;
  highlights: string[];
  caution: string[];
  nextSteps: string[];
};

const SIMULATION_DEFAULTS: SimulationDraft = {
  evidenceFilter: '',
  scenarioPreset: 'balanced',
  parentTone: 58,
  escalation: 46,
  persistence: 62,
  adminSupport: 52,
  publicSpread: 24,
  legalLeverage: 38,
  goal: 'stabilize',
};

const simulationPresetMap: Record<SimulationPreset, Partial<SimulationDraft>> = {
  shield: {
    scenarioPreset: 'shield',
    parentTone: 64,
    escalation: 52,
    persistence: 72,
    adminSupport: 58,
    publicSpread: 18,
    legalLeverage: 44,
    goal: 'document',
  },
  balanced: {
    scenarioPreset: 'balanced',
    parentTone: 58,
    escalation: 46,
    persistence: 62,
    adminSupport: 52,
    publicSpread: 24,
    legalLeverage: 38,
    goal: 'stabilize',
  },
  assertive: {
    scenarioPreset: 'assertive',
    parentTone: 70,
    escalation: 66,
    persistence: 82,
    adminSupport: 48,
    publicSpread: 38,
    legalLeverage: 62,
    goal: 'escalate',
  },
};

const clampSimulationNumber = (value: unknown, fallback: number) => {
  const n = Number(value);
  if (!Number.isFinite(n)) return fallback;
  return Math.max(0, Math.min(100, Math.round(n)));
};

const ensureSimulationDraft = (): SimulationDraft => {
  const draft = ((ui as any).simulationDraft || ((ui as any).simulationDraft = { ...SIMULATION_DEFAULTS })) as SimulationDraft;
  draft.evidenceFilter = String(draft.evidenceFilter || '');
  draft.scenarioPreset = (['shield', 'balanced', 'assertive'].includes(String(draft.scenarioPreset || '')) ? draft.scenarioPreset : 'balanced') as SimulationPreset;
  draft.parentTone = clampSimulationNumber(draft.parentTone, SIMULATION_DEFAULTS.parentTone);
  draft.escalation = clampSimulationNumber(draft.escalation, SIMULATION_DEFAULTS.escalation);
  draft.persistence = clampSimulationNumber(draft.persistence, SIMULATION_DEFAULTS.persistence);
  draft.adminSupport = clampSimulationNumber(draft.adminSupport, SIMULATION_DEFAULTS.adminSupport);
  draft.publicSpread = clampSimulationNumber(draft.publicSpread, SIMULATION_DEFAULTS.publicSpread);
  draft.legalLeverage = clampSimulationNumber(draft.legalLeverage, SIMULATION_DEFAULTS.legalLeverage);
  draft.goal = (['stabilize', 'document', 'escalate'].includes(String(draft.goal || '')) ? draft.goal : 'stabilize') as SimulationDraft['goal'];
  return draft;
};

const getSimulationBaseRecords = () => {
  return S.records.slice().sort((a, b) => String(b.ts || '').localeCompare(String(a.ts || '')));
};

const getSimulationVisibleRecords = () => {
  const draft = ensureSimulationDraft();
  const keyword = String(draft.evidenceFilter || '').trim().toLowerCase();
  const baseRecords = getSimulationBaseRecords();
  if (!keyword) return baseRecords;
  return baseRecords.filter((record: any) => [
    String(record.summary || ''),
    String(record.ts || ''),
    String(record.storeType || ''),
    String(record.storeOther || ''),
    String(record.place || ''),
    String(record.placeOther || ''),
  ].join(' ').toLowerCase().includes(keyword));
};

const getSimulationCaseRecordIds = (caseId: string | null | undefined) => {
  const safeId = String(caseId || '').trim();
  if (!safeId || !S.cases[safeId]) return [] as string[];
  return recordsForCase(S.records, S.cases[safeId]).map((record: any) => String(record.id || '')).filter(Boolean);
};

const applySimulationCaseSelection = (caseId: string | null | undefined) => {
  const safeId = String(caseId || '').trim();
  (ui as any).simulationCaseId = safeId && S.cases[safeId] ? safeId : null;
  if ((ui as any).simulationCaseId) {
    (ui as any).simulationSelectedRecordIds = getSimulationCaseRecordIds((ui as any).simulationCaseId);
  }
  (ui as any).simulationResult = null;
  markSimulationDirty();
};

const reseedSimulationSelection = (force = false) => {
  const baseRecords = getSimulationBaseRecords();
  const allowed = new Set(baseRecords.map((record: any) => String(record.id || '')));
  let selected = Array.isArray((ui as any).simulationSelectedRecordIds)
    ? ((ui as any).simulationSelectedRecordIds as string[]).map((id) => String(id || '').trim()).filter((id) => allowed.has(id))
    : [];
  if (force || !selected.length) {
    selected = baseRecords.slice(0, 5).map((record: any) => String(record.id || ''));
  }
  (ui as any).simulationSelectedRecordIds = selected;
  return selected;
};

const markSimulationDirty = () => {
  (ui as any).simulationDirty = true;
};

let _simulationSearchRenderTimer: number | null = null;
const scheduleSimulationSearchRender = () => {
  if (_simulationSearchRenderTimer) window.clearTimeout(_simulationSearchRenderTimer);
  _simulationSearchRenderTimer = window.setTimeout(() => {
    _simulationSearchRenderTimer = null;
    render();
  }, 160);
};

const getSimulationPickerSelectedIds = () => Array.isArray((ui as any).simulationPickerSelectedRecordIds)
  ? ((ui as any).simulationPickerSelectedRecordIds as string[]).map((id) => String(id || '').trim()).filter(Boolean)
  : [] as string[];

const setSimulationPickerSelectedIds = (ids: string[]) => {
  const allowed = new Set(getSimulationBaseRecords().map((record: any) => String(record.id || '')));
  const unique: string[] = [];
  for (const rawId of ids || []) {
    const safeId = String(rawId || '').trim();
    if (!safeId || !allowed.has(safeId) || unique.includes(safeId)) continue;
    unique.push(safeId);
  }
  (ui as any).simulationPickerSelectedRecordIds = unique;
  return unique;
};

const syncSimulationPickerSelectionFromLive = () => {
  const liveIds = Array.isArray((ui as any).simulationSelectedRecordIds)
    ? ((ui as any).simulationSelectedRecordIds as string[]).map((id) => String(id || '').trim()).filter(Boolean)
    : [] as string[];
  return setSimulationPickerSelectedIds(liveIds);
};

const applySimulationPreset = (preset: SimulationPreset) => {
  const draft = ensureSimulationDraft();
  Object.assign(draft, simulationPresetMap[preset] || simulationPresetMap.balanced);
  draft.scenarioPreset = preset;
};

const computeSimulationResult = (): SimulationResult | null => {
  const draft = ensureSimulationDraft();
  const baseRecords = getSimulationBaseRecords();
  const selectedIds = reseedSimulationSelection(false);
  const selectedRecords = baseRecords.filter((record: any) => selectedIds.includes(String(record.id || '')));
  if (!selectedRecords.length) return null;

  const uniqueStores = new Set(selectedRecords.map((record: any) => `${String(record.storeType || '')}:${String(record.storeOther || '')}`)).size;
  const trustedCount = selectedRecords.filter((record: any) => {
    const integrity = verifyRecordIntegrity(ensureRecordV8(record) as any) as any;
    return !!integrity?.valid || !!integrity?.trusted;
  }).length;
  const highLvCount = selectedRecords.filter((record: any) => ['LV3', 'LV4', 'LV5'].includes(String(record.lv || ''))).length;
  const documentCount = selectedRecords.filter((record: any) => ['문서', '공문', '가정통신문', '회의록', '상담록', '상담일지', '지도일지'].includes(String(record.storeType || ''))).length;
  const voiceCount = selectedRecords.filter((record: any) => ['통화녹취', '음성녹음', '전화', '방문상담'].includes(String(record.storeType || ''))).length;
  const actionCount = selectedRecords.filter((record: any) => /조치|안내|연락|면담|상담|보고/.test(String(record.summary || ''))).length;

  const evidencePower = Math.max(18, Math.min(97, Math.round(30 + selectedRecords.length * 7 + uniqueStores * 5 + trustedCount * 4 + highLvCount * 3 + documentCount * 2)));
  const counterLogic = Math.max(16, Math.min(96, Math.round(28 + documentCount * 7 + actionCount * 5 + uniqueStores * 4 + draft.adminSupport * 0.22 - draft.parentTone * 0.08 - draft.escalation * 0.04)));
  const escalationRisk = Math.max(14, Math.min(96, Math.round(18 + draft.parentTone * 0.20 + draft.escalation * 0.36 + draft.persistence * 0.18 + draft.publicSpread * 0.30 + draft.legalLeverage * 0.14 - trustedCount * 2 - documentCount * 2 - draft.adminSupport * 0.14)));
  const communicationControl = Math.max(18, Math.min(96, Math.round(26 + draft.adminSupport * 0.34 + selectedRecords.length * 4 + documentCount * 3 + voiceCount * 2 - draft.persistence * 0.08 - draft.publicSpread * 0.10)));

  let responseIndex = Math.round(evidencePower * 0.34 + counterLogic * 0.29 + communicationControl * 0.24 + (100 - escalationRisk) * 0.13);
  if (draft.goal === 'document') responseIndex += 4;
  if (draft.goal === 'escalate') responseIndex -= 3;
  responseIndex = Math.max(18, Math.min(97, responseIndex));

  const recommendedTone = escalationRisk >= 70
    ? '저자극 · 서면집중'
    : responseIndex >= 78
      ? '차분하지만 단호하게'
      : draft.goal === 'document'
        ? '사실확인 우선 · 기록축적형'
        : '관계회복 여지 확보형';

  const recommendedAction = escalationRisk >= 72
    ? '감정적 응답을 멈추고 공식 채널과 제출용 기록 정리로 전환하세요.'
    : counterLogic >= 70
      ? '핵심 사실표와 기록 인덱스를 먼저 보내고, 추가 연락은 한 창구로 모으세요.'
      : '기록 로그를 더 모아 설명 축을 보강한 뒤 대응 문구를 정리하세요.';

  const highlights = [
    documentCount >= 2 ? '문서 축이 있어 공유/제출 문서 전환 흐름이 자연스럽습니다.' : '',
    trustedCount >= 2 ? '봉인/무결성 흔적이 남은 기록이 포함되어 신뢰 확보에 유리합니다.' : '',
    uniqueStores >= 3 ? '녹취·문서·대화기록처럼 채널이 분산되어 있어 흐름 설명에 좋습니다.' : '',
    actionCount >= 2 ? '선행 대응 흔적이 남아 있어 경과 설명과 타임라인 정리에 유리합니다.' : '',
  ].filter(Boolean).slice(0, 3);

  const caution = [
    selectedRecords.length < 3 ? '선택한 기록 수가 적어 설명 논리가 짧게 끊길 수 있습니다.' : '',
    escalationRisk >= 68 ? '확산 가능성이 높아 개별 DM·전화 대응을 줄이는 편이 안전합니다.' : '',
    draft.adminSupport < 45 ? '공유 대상과 보고선 정리가 약해 단독 대응처럼 보일 수 있습니다.' : '',
    draft.publicSpread >= 45 ? '커뮤니티/단체방 확산 가정이 높아 표현 수위를 더 낮출 필요가 있습니다.' : '',
  ].filter(Boolean).slice(0, 3);

  const nextSteps = [
    counterLogic >= 68 ? '1차 회신은 사실표 3줄과 기록 번호 묶음으로 짧게 보내기' : '누락된 사실관계와 날짜축을 먼저 보강하기',
    escalationRisk >= 68 ? '관련자와 동일 문구를 공유해 응답 창구를 1개로 묶기' : '연락·안내·메모 로그를 한 장의 타임라인으로 정리하기',
    responseIndex >= 74 ? '공유/제출 문서로 전환할 기록 묶음을 따로 저장하기' : '근거가 약한 부분은 추가 기록 확보 전까지 답변 범위를 제한하기',
  ];

  const bundleLabel = draft.scenarioPreset === 'shield'
    ? '완충형 묶음'
    : draft.scenarioPreset === 'assertive'
      ? '주도형 묶음'
      : '균형형 묶음';

  return {
    calculatedAt: nowISO(),
    bundleLabel,
    selectedCount: selectedRecords.length,
    totalAvailable: baseRecords.length,
    responseIndex,
    evidencePower,
    counterLogic,
    escalationRisk,
    communicationControl,
    recommendedTone,
    recommendedAction,
    highlights,
    caution,
    nextSteps,
  };
};


type StrategyChatRole = 'assistant' | 'user' | 'system';
type StrategyChatMode = 'analysis' | 'record';
type StrategyChatMessage = {
  id: string;
  role: StrategyChatRole;
  content: string;
  ts: string;
  meta?: string;
  mode?: StrategyChatMode;
  recordDraft?: boolean;
  sourceInput?: string;
  relatedRecordIds?: string[];
  relatedCollectionTitle?: string;
};

type StrategyChatInvokeResult = {
  answer: string;
  modelPath: string;
  runner: string;
  promptChars: number;
  recordsUsed: number;
  perfMetrics?: StrategyPerfMetrics;
  retrievalQuery?: string;
  evidencePacket?: {
    mode?: string;
    caseTitle?: string;
    focusSummary?: string;
    overview?: string;
    actorSummary?: string[];
    timelineSummary?: string[];
    riskSummary?: string[];
    gaps?: string[];
    evidenceRecords?: Array<{
      refId?: string;
      recordId?: string;
      ts?: string;
      actor?: string;
      place?: string;
      store?: string;
      summary?: string;
      score?: number;
      riskLabel?: string;
      reasons?: string[];
    }>;
    legalReferences?: Array<{
      refId?: string;
      lawId?: string;
      lawName?: string;
      shortName?: string;
      articleRef?: string;
      articleTitle?: string;
      legalPoint?: string;
      teacherUseCase?: string;
      sourceUrl?: string;
      statusLabel?: string;
      relevanceReasons?: string[];
    }>;
  };
};

type StrategyPrewarmInvokeResult = {
  backendKind: string;
  ready: boolean;
  reason: string;
  hyperclovaEndpoint: string;
  roosyEndpoint: string;
};

type StrategyPerfMetrics = {
  mode: string;
  runId: string;
  promptHash: string;
  caseHash: string;
  recordsHash: string;
  modelConfigHash: string;
  backendKind: string;
  cacheRequested: boolean;
  cacheLoaded: boolean;
  cacheApplied: boolean;
  requestedMode: string;
  appliedMode: string;
  bypassReason: string;
  totalE2eMs: number;
  ttftMs: number | null;
  totalPromptTokens: number;
  totalOutputTokens: number;
  e2eTps: number;
  decodeTps: number;
  finalStageTps: number;
  fullDraceAppliedStages: string[];
  peakMemoryMb: number;
  stageExecutionSumMs: number;
  orchestrationOverheadMs: number;
  promptBuildMs: number;
  cacheCapabilityMs: number;
  cachePlanMs: number;
  cacheLookupMs: number;
  promptFileWriteMs: number;
  processSpawnMs: number;
  stdoutReadMs: number;
  postprocessMs: number;
  otherOverheadMs: number;
  stages: Array<{
    stageName: string;
    modelId: string;
    backendKind: string;
    runnerPath: string;
    threads: number;
    threadsBatch: number;
    nCtx: number;
    maxTokens: number;
    temperature: number;
    topP: number;
    repeatPenalty: number;
    e2eMs: number;
    ttftMs: number | null;
    promptTokens: number;
    outputTokens: number;
    promptEvalMs: number | null;
    decodeMs: number | null;
    e2eTps: number;
    decodeTps: number;
    peakMemoryMb: number;
    processSpawnMs: number;
    promptFileWriteMs: number;
    stdoutReadMs: number;
    postprocessMs: number;
    cache: {
      cacheRequested: boolean;
      cacheLoaded: boolean;
      cacheEnabled: boolean;
      cacheSupported: boolean;
      cacheWarm: boolean;
      cacheApplied: boolean;
      syntheticCacheRequested: boolean;
      syntheticCacheSupported: boolean;
      syntheticCacheApplied: boolean;
      cacheModeRequested: string;
      cacheModeApplied: string;
      bypassReason: string;
      draftProvider: string;
      promptTokenCacheSupported: boolean;
      promptTokenCacheLoaded: boolean;
      promptTokenCacheApplied: boolean;
      promptTokenCacheHitRatio: number;
      prefixKvSupported: boolean;
      prefixKvApplied: boolean;
      prefixReusedTokens: number;
      prefixTotalTokens: number;
      prefixReuseRatio: number;
      kvLoadMs: number;
      kvSaveMs: number;
      tokenCacheSupported: boolean;
      tokenCacheLoaded: boolean;
      tokenCacheApplied: boolean;
      tokenVerificationSupported: boolean;
      tokenCacheLookupMs: number;
      proposedTokens: number;
      acceptedTokens: number;
      rejectedTokens: number;
      acceptanceRatio: number;
      verifyBatches: number;
      rejectedBatches: number;
      avgProposedBatchSize: number;
      avgAcceptedBatchSize: number;
      acceptedTokensPerVerify: number;
      fallbackTokens: number;
      fallbackDecodeTokens: number;
      rendererInsertedTokens: number;
      llmGeneratedTokens: number;
      outputTokenReductionRatio: number;
    };
  }>;
  cacheSummary: {
    backendType: string;
    cacheRequested: boolean;
    cacheLoaded: boolean;
    cacheApplied: boolean;
    cacheSupported: boolean;
    cacheWarm: boolean;
    syntheticCacheRequested: boolean;
    syntheticCacheSupported: boolean;
    syntheticCacheApplied: boolean;
    cacheModeRequested: string;
    cacheModeApplied: string;
    bypassReason: string;
    draftProvider: string;
    promptTokenCacheSupported: boolean;
    promptTokenCacheLoaded: boolean;
    promptTokenCacheApplied: boolean;
    promptTokenCacheHitRatio: number;
    prefixKvSupported: boolean;
    prefixKvApplied: boolean;
    prefixReusedTokens: number;
    prefixTotalTokens: number;
    prefixReuseRatio: number;
    tokenCacheSupported: boolean;
    tokenCacheLoaded: boolean;
    tokenCacheApplied: boolean;
    tokenVerificationSupported: boolean;
    proposedTokens: number;
    acceptedTokens: number;
    rejectedTokens: number;
    acceptanceRatio: number;
    verifyBatches: number;
    rejectedBatches: number;
    avgProposedBatchSize: number;
    avgAcceptedBatchSize: number;
    acceptedTokensPerVerify: number;
    fallbackTokens: number;
    fallbackDecodeTokens: number;
    rendererInsertedTokens: number;
    llmGeneratedTokens: number;
    outputTokenReductionRatio: number;
  };
};

type StrategyPerfLabSample = StrategyPerfMetrics & {
  comparisonKey: string;
  capturedAt: string;
  sampleSize: number;
  e2eP50Ms: number;
  e2eP95Ms: number;
  ttftP50Ms: number;
  ttftP95Ms: number;
  tpsP50: number;
  tpsP95: number;
  peakMemoryP95Mb: number;
  benchmarkPhase: 'measured' | 'warmup';
};

type StrategyPerfLabState = {
  baseline: StrategyPerfLabSample | null;
  drace: StrategyPerfLabSample | null;
  baselineRuns: StrategyPerfLabSample[];
  draceRuns: StrategyPerfLabSample[];
  draceWarmup: StrategyPerfLabSample | null;
  latest: StrategyPerfLabSample | null;
  comparisonWarning?: string;
};

type StrategyPerfRunState = {
  status: 'idle' | 'running' | 'succeeded' | 'failed';
  requestedBackend: string;
  requestedMode: string;
  actualBackend: string;
  startedAt: string;
  finishedAt: string;
  error: string;
  cacheApplied: boolean;
};

const emptyStrategyPerfLabState = (): StrategyPerfLabState => ({
  baseline: null,
  drace: null,
  baselineRuns: [],
  draceRuns: [],
  draceWarmup: null,
  latest: null,
  comparisonWarning: '',
});

const emptyStrategyPerfRunState = (): StrategyPerfRunState => ({
  status: 'idle',
  requestedBackend: 'llama-server',
  requestedMode: 'FullDRACE',
  actualBackend: '',
  startedAt: '',
  finishedAt: '',
  error: '',
  cacheApplied: false,
});

type StrategyThreadPackageState = {
  id: string;
  title: string;
  summary: string;
  createdAt: string;
  updatedAt: string;
  caseId: string | null;
  caseTitle: string;
  selectedRecordIds: string[];
  retrievalQuery: string;
  messages: StrategyChatMessage[];
  evidencePacket: StrategyChatInvokeResult['evidencePacket'] | null;
};

type StrategyChatProgressPayload = {
  stage?: string;
  message?: string;
};

const STRATEGY_DEFAULT_PROMPT = '이 상황에서 지금 어떤 말부터 꺼내고 무엇을 먼저 남겨야 할까요?';
const STRATEGY_PROGRESS_MAX = 8;
const STRATEGY_THREAD_PACKAGE_LIMIT = 24;
let _strategyProgressListenerBound = false;
let _strategyBackendPrewarmStarted = false;

const getStrategyChatMode = (): StrategyChatMode => (((ui as any).strategyChatMode === 'record') ? 'record' : 'analysis');

const getStrategyChatMessages = (): StrategyChatMessage[] => {
  const raw = Array.isArray((ui as any).strategyChatMessages) ? (ui as any).strategyChatMessages : [];
  (ui as any).strategyChatMessages = raw;
  return raw as StrategyChatMessage[];
};

const getStrategyProgressLines = (): string[] => {
  const raw = Array.isArray((ui as any).strategyChatProgressLines) ? (ui as any).strategyChatProgressLines : [];
  (ui as any).strategyChatProgressLines = raw;
  return raw as string[];
};

const getStrategyThreadPackages = (): StrategyThreadPackageState[] => {
  const raw = Array.isArray((S as any).strategyThreadPackages) ? (S as any).strategyThreadPackages : [];
  (S as any).strategyThreadPackages = raw;
  return raw as StrategyThreadPackageState[];
};

const getActiveStrategyThreadPackageId = () => String((ui as any).strategyThreadPackageId || '').trim();

const findStrategyThreadPackage = (id: string) => getStrategyThreadPackages().find((item) => String(item.id || '') === String(id || '').trim()) || null;

const cloneStrategyChatMessages = (messages = getStrategyChatMessages()): StrategyChatMessage[] => messages.map((item) => ({
  id: String(item.id || uid('stratmsg')),
  role: item.role === 'user' || item.role === 'system' ? item.role : 'assistant',
  content: String(item.content || '').trim(),
  ts: String(item.ts || nowISO()),
  ...(String(item.meta || '').trim() ? { meta: String(item.meta || '').trim() } : {}),
  ...((((item as any).mode === 'record') ? { mode: 'record' as const } : { mode: 'analysis' as const })),
  ...((item as any).recordDraft ? { recordDraft: true } : {}),
}));

const cloneStrategyEvidencePacket = (packet: StrategyChatInvokeResult['evidencePacket'] | null | undefined): StrategyChatInvokeResult['evidencePacket'] | null => {
  if (!packet) return null;
  try {
    return JSON.parse(JSON.stringify(packet)) as StrategyChatInvokeResult['evidencePacket'];
  } catch {
    return null;
  }
};

const getStrategyCurrentCase = () => {
  const caseId = String((ui as any).simulationCaseId || '').trim();
  return caseId && S.cases[caseId] ? S.cases[caseId] as CaseItem : null;
};

const getStrategySelectedRecordIds = () => Array.isArray((ui as any).simulationSelectedRecordIds)
  ? ((ui as any).simulationSelectedRecordIds as string[]).map((id) => String(id || '').trim()).filter(Boolean)
  : [] as string[];

const buildStrategyThreadPackageTitle = (messages: StrategyChatMessage[], caseTitle: string) => {
  const firstUser = messages.find((item) => item.role === 'user' && String(item.content || '').trim());
  const seed = String(firstUser?.content || '').trim();
  if (caseTitle && seed) return `${trunc(caseTitle, 16)} · ${trunc(seed, 18)}`;
  if (seed) return trunc(seed, 28);
  if (caseTitle) return `${trunc(caseTitle, 24)} 스레드`;
  return `사건분석 스레드 ${new Date().toLocaleDateString('ko-KR', { month: '2-digit', day: '2-digit' })}`;
};

const buildStrategyThreadPackageSummary = (
  messages: StrategyChatMessage[],
  evidencePacket: StrategyChatInvokeResult['evidencePacket'] | null,
  retrievalQuery: string,
) => {
  const lastAssistant = [...messages].reverse().find((item) => item.role === 'assistant' && String(item.content || '').trim());
  const lastUser = [...messages].reverse().find((item) => item.role === 'user' && String(item.content || '').trim());
  const focus = String(evidencePacket?.focusSummary || '').trim();
  const overview = String(evidencePacket?.overview || '').trim();
  return trunc(lastAssistant?.content || lastUser?.content || focus || retrievalQuery || overview || '저장된 대화 패키지', 72);
};

const snapshotStrategyThreadPackage = (existing?: StrategyThreadPackageState | null): StrategyThreadPackageState => {
  const messages = cloneStrategyChatMessages().filter((item) => item.content);
  const currentCase = getStrategyCurrentCase();
  const caseTitle = String(currentCase?.title || existing?.caseTitle || '').trim();
  const retrievalQuery = String((ui as any).strategyChatRetrievalQuery || existing?.retrievalQuery || '').trim();
  const evidencePacket = cloneStrategyEvidencePacket(((ui as any).strategyChatEvidencePacket || existing?.evidencePacket || null) as any);
  return {
    id: String(existing?.id || uid('threadpkg')),
    title: String(existing?.title || buildStrategyThreadPackageTitle(messages, caseTitle)).trim(),
    summary: buildStrategyThreadPackageSummary(messages, evidencePacket, retrievalQuery),
    createdAt: String(existing?.createdAt || nowISO()),
    updatedAt: nowISO(),
    caseId: currentCase?.id || existing?.caseId || null,
    caseTitle,
    selectedRecordIds: getStrategySelectedRecordIds().slice(0, 24),
    retrievalQuery,
    messages,
    evidencePacket,
  };
};

const persistStrategyThreadPackage = async (
  existingId?: string | null,
  opts: { silent?: boolean; activate?: boolean } = {},
) => {
  const hasContent = getStrategyChatMessages().some((item) => String(item.content || '').trim())
    || !!String((ui as any).strategyChatRetrievalQuery || '').trim()
    || !!(ui as any).strategyChatEvidencePacket;
  if (!hasContent) {
    if (!opts.silent) toast('먼저 저장할 대화나 근거 묶음을 만들어주세요');
    return null;
  }
  const packages = getStrategyThreadPackages();
  const current = existingId ? findStrategyThreadPackage(existingId) : null;
  const snapshot = snapshotStrategyThreadPackage(current);
  const next = packages.filter((item) => item.id !== snapshot.id);
  next.unshift(snapshot);
  (S as any).strategyThreadPackages = next.slice(0, STRATEGY_THREAD_PACKAGE_LIMIT);
  if (opts.activate !== false) (ui as any).strategyThreadPackageId = snapshot.id;
  await saveState(S);
  if (!opts.silent) toast(current ? '사건분석 패키지를 업데이트했어요' : '현재 대화를 패키지로 저장했어요');
  return snapshot;
};

const syncActiveStrategyThreadPackage = () => {
  const activeId = getActiveStrategyThreadPackageId();
  if (!activeId) return;
  void persistStrategyThreadPackage(activeId, { silent: true, activate: true });
};

const openStrategyThreadPackage = async (id: string) => {
  const pkg = findStrategyThreadPackage(id);
  if (!pkg) return;
  (ui as any).strategyThreadPackageId = pkg.id;
  (ui as any).strategyChatMessages = cloneStrategyChatMessages(pkg.messages || []);
  (ui as any).strategyChatMode = (Array.isArray(pkg.messages) && pkg.messages.some((item) => String((item as any).mode || '') === 'record' || !!(item as any).recordDraft))
    ? 'record'
    : 'analysis';
  (ui as any).strategyChatInput = '';
  (ui as any).strategyChatCaret = -1;
  (ui as any).strategyChatError = '';
  (ui as any).strategyRecordEditMessageId = '';
  (ui as any).strategyRecordEditDraft = '';
  (ui as any).strategyChatPending = false;
  (ui as any).strategyChatEvidencePacket = cloneStrategyEvidencePacket(pkg.evidencePacket);
  (ui as any).strategyChatRetrievalQuery = String(pkg.retrievalQuery || '').trim();
  clearStrategyChatProgress();
  if (pkg.caseId && S.cases[pkg.caseId]) {
    (ui as any).simulationCaseId = pkg.caseId;
  }
  (ui as any).simulationSelectedRecordIds = Array.isArray(pkg.selectedRecordIds)
    ? pkg.selectedRecordIds.map((item) => String(item || '').trim()).filter(Boolean)
    : [];
  (ui as any).simulationResult = null;
  markSimulationDirty();
  closeStrategyMentionMenu();
  S.tab = 'legal' as any;
  (ui as any).legalTab = 'simulation';
  await saveState(S);
  render();
  toast('사건분석 스레드 패키지를 열었어요');
};

const startFreshStrategyThread = () => {
  (ui as any).strategyThreadPackageId = '';
  clearStrategyChat();
  render();
  toast('새 사건분석 스레드를 시작했어요');
};

const deleteStrategyThreadPackage = async (id: string) => {
  const pkg = findStrategyThreadPackage(id);
  if (!pkg) return;
  if (!(await openConfirm(`"${String(pkg.title || '사건분석 스레드')}" 패키지를 삭제할까요?`))) return;
  (S as any).strategyThreadPackages = getStrategyThreadPackages().filter((item) => item.id !== pkg.id);
  if (getActiveStrategyThreadPackageId() === pkg.id) {
    (ui as any).strategyThreadPackageId = '';
    clearStrategyChat();
  }
  await saveState(S);
  render();
  toast('사건분석 패키지를 삭제했어요');
};

const clearStrategyChatProgress = () => {
  (ui as any).strategyChatProgressLines = [];
  (ui as any).strategyChatProgressStage = '';
};

const percentileNumber = (values: number[], p: number) => {
  const filtered = values.map((v) => Number(v || 0)).filter((v) => v > 0).sort((a, b) => a - b);
  if (!filtered.length) return 0;
  const idx = Math.min(filtered.length - 1, Math.max(0, Math.round((filtered.length - 1) * p)));
  return filtered[idx];
};

const averageNumber = (values: number[]) => {
  const filtered = values.map((v) => Number(v || 0)).filter((v) => Number.isFinite(v));
  if (!filtered.length) return 0;
  return filtered.reduce((sum, value) => sum + value, 0) / filtered.length;
};

const buildStrategyPerfStageSignature = (metrics: StrategyPerfMetrics) => (Array.isArray(metrics.stages) ? metrics.stages : [])
  .map((stage) => [
    String(stage.stageName || ''),
    String(stage.modelId || ''),
    Number(stage.nCtx || 0),
    Number(stage.maxTokens || 0),
    Number(stage.threads || 0),
    Number(stage.temperature || 0),
    Number(stage.topP || 0),
    Number(stage.repeatPenalty || 0),
  ].join(':'))
  .join('|');

const buildStrategyPerfComparisonKey = (metrics: StrategyPerfMetrics) => [
  String(metrics.mode || ''),
  String(metrics.promptHash || ''),
  String(metrics.caseHash || ''),
  String(metrics.recordsHash || ''),
  String(metrics.modelConfigHash || ''),
  buildStrategyPerfStageSignature(metrics),
].join('|');

const aggregateStrategyPerfRuns = (runs: StrategyPerfLabSample[]): StrategyPerfLabSample | null => {
  if (!runs.length) return null;
  const latest = runs[runs.length - 1];
  const e2eValues = runs.map((run) => run.totalE2eMs || 0);
  const ttftValues = runs.map((run) => run.ttftMs || 0);
  const tpsValues = runs.map((run) => run.e2eTps || 0);
  const memoryValues = runs.map((run) => run.peakMemoryMb || 0);
  return {
    ...latest,
    totalE2eMs: averageNumber(e2eValues),
    ttftMs: averageNumber(ttftValues) || null,
    e2eTps: averageNumber(tpsValues),
    peakMemoryMb: averageNumber(memoryValues),
    sampleSize: runs.length,
    e2eP50Ms: percentileNumber(e2eValues, 0.5),
    e2eP95Ms: percentileNumber(e2eValues, 0.95),
    ttftP50Ms: percentileNumber(ttftValues, 0.5),
    ttftP95Ms: percentileNumber(ttftValues, 0.95),
    tpsP50: percentileNumber(tpsValues, 0.5),
    tpsP95: percentileNumber(tpsValues, 0.95),
    peakMemoryP95Mb: percentileNumber(memoryValues, 0.95),
    benchmarkPhase: 'measured',
  };
};

const formatStrategyRunnerLabel = (runner: unknown) => {
  const raw = String(runner || '').trim();
  const file = raw.split(/[\\/]/).pop() || raw;
  if (!file) return '로컬 추론기';
  if (/llama-(sidecar|cli|server)/i.test(file)) return '로컬 추론기';
  return file;
};

const recordStrategyPerfSample = (metrics: StrategyPerfMetrics | null | undefined) => {
  if (!metrics) return;
  const sample: StrategyPerfLabSample = {
    ...metrics,
    comparisonKey: buildStrategyPerfComparisonKey(metrics),
    capturedAt: nowISO(),
    sampleSize: 1,
    e2eP50Ms: Number(metrics.totalE2eMs || 0),
    e2eP95Ms: Number(metrics.totalE2eMs || 0),
    ttftP50Ms: Number(metrics.ttftMs || 0),
    ttftP95Ms: Number(metrics.ttftMs || 0),
    tpsP50: Number(metrics.e2eTps || 0),
    tpsP95: Number(metrics.e2eTps || 0),
    peakMemoryP95Mb: Number(metrics.peakMemoryMb || 0),
    benchmarkPhase: 'measured',
  };
  const current = (((ui as any).strategyPerfLab || emptyStrategyPerfLabState()) as StrategyPerfLabState);
  const next: StrategyPerfLabState = {
    baseline: current.baseline,
    drace: current.drace,
    baselineRuns: Array.isArray(current.baselineRuns) ? [...current.baselineRuns] : [],
    draceRuns: Array.isArray(current.draceRuns) ? [...current.draceRuns] : [],
    draceWarmup: current.draceWarmup || null,
    latest: sample,
    comparisonWarning: '',
  };
  const cacheRequested = !!metrics.cacheSummary?.cacheRequested;
  const cacheApplied = !!metrics.cacheSummary?.cacheApplied;
  const cacheWarm = !!metrics.cacheSummary?.cacheWarm;
  const isWarmup = cacheRequested && cacheApplied && !cacheWarm;
  const currentSideRuns = cacheRequested ? next.draceRuns : next.baselineRuns;
  const currentSideKey = currentSideRuns[0]?.comparisonKey || (cacheRequested ? next.drace?.comparisonKey : next.baseline?.comparisonKey) || '';
  if (currentSideKey && currentSideKey !== sample.comparisonKey) {
    if (cacheRequested) {
      next.draceRuns = [];
      next.drace = null;
      next.draceWarmup = null;
    } else {
      next.baselineRuns = [];
      next.baseline = null;
    }
  }
  const otherKey = cacheRequested ? current.baseline?.comparisonKey : current.drace?.comparisonKey;
  if (otherKey && otherKey !== sample.comparisonKey) {
    next.comparisonWarning = '비교 기준 불일치: 같은 입력과 설정으로 다시 측정하세요.';
  }
  if (cacheRequested) {
    if (isWarmup) {
      next.draceWarmup = { ...sample, benchmarkPhase: 'warmup' };
    } else {
      next.draceRuns.push(sample);
      next.draceRuns = next.draceRuns.slice(-5);
      next.drace = aggregateStrategyPerfRuns(next.draceRuns);
    }
  } else {
    next.baselineRuns.push(sample);
    next.baselineRuns = next.baselineRuns.slice(-3);
    next.baseline = aggregateStrategyPerfRuns(next.baselineRuns);
  }
  const baseline = next.baseline;
  const drace = next.drace;
  if (
    baseline &&
    drace &&
    baseline.comparisonKey === drace.comparisonKey &&
    drace.cacheRequested &&
    !drace.cacheApplied
  ) {
    const base = Number(baseline.totalE2eMs || 0);
    const currentE2e = Number(drace.totalE2eMs || 0);
    if (base > 0 && currentE2e > base * 1.05) {
      const overheadDeltaMs = Math.round(currentE2e - base);
      next.comparisonWarning = 'DRACE cache was requested but not applied. Runtime should match baseline. Instrumentation overhead exceeded 5%.';
      log('drace regression warning', {
        baseline_e2e_ms: base,
        current_e2e_ms: currentE2e,
        overhead_delta_ms: overheadDeltaMs,
        stage_execution_sum_ms: drace.stageExecutionSumMs,
        orchestration_overhead_ms: drace.orchestrationOverheadMs,
        bypass_reason: drace.bypassReason,
      });
    }
  }
  (ui as any).strategyPerfLab = next;
  persistStrategyPerfLabState(next);
};

const normalizeStrategyModel = (_value: unknown) => 'roosy-hybrid' as const;

const getStrategyBackendMode = (): StrategyBackendMode => 'llama-server';

const maybeStartStrategyBackendPrewarm = (reason = 'startup') => {
  if (_strategyBackendPrewarmStarted) return;
  _strategyBackendPrewarmStarted = true;
  const llamaServer = getStrategyLlamaServerConfig();
  void invoke('strategy_prewarm_backend', {
    args: {
      backendMode: 'llama-server',
      llamaServer,
    },
  })
    .then((result) => {
      const prewarm = result as StrategyPrewarmInvokeResult;
      if (!prewarm?.ready) {
        _strategyBackendPrewarmStarted = false;
      }
      log('strategy backend prewarm', {
        reason,
        backendKind: prewarm?.backendKind,
        ready: prewarm?.ready,
        result: prewarm,
      });
    })
    .catch((err) => {
      _strategyBackendPrewarmStarted = false;
      log('strategy backend prewarm failed', { reason, err });
    });
};

const getStrategyLlamaServerConfig = () => {
  const raw = ((ui as any).strategyLlamaServerConfig || defaultStrategyLlamaServerConfig()) as StrategyLlamaServerConfigDraft;
  const isManagedLoopbackCompletion = (value: string) => {
    const trimmed = String(value || '').trim();
    return /^https?:\/\/(127\.0\.0\.1|localhost):\d+\/completion$/i.test(trimmed);
  };
  const normalizeSlot = (value: string) => {
    const trimmed = String(value || '').trim();
    if (!trimmed) return null;
    const parsed = Number(trimmed);
    return Number.isFinite(parsed) && parsed >= 0 ? Math.floor(parsed) : null;
  };
  const normalizeMs = (value: string, fallback: number) => {
    const parsed = Number(String(value || '').trim());
    return Number.isFinite(parsed) && parsed > 0 ? Math.floor(parsed) : fallback;
  };
  let hyperclovaUrl = String(raw.hyperclovaUrl || '').trim() || defaultStrategyLlamaServerConfig().hyperclovaUrl;
  let roosyUrl = String(raw.roosyUrl || '').trim() || defaultStrategyLlamaServerConfig().roosyUrl;
  if (
    hyperclovaUrl.localeCompare(roosyUrl, undefined, { sensitivity: 'accent' }) === 0
    && isManagedLoopbackCompletion(hyperclovaUrl)
  ) {
    hyperclovaUrl = defaultStrategyLlamaServerConfig().hyperclovaUrl;
    roosyUrl = defaultStrategyLlamaServerConfig().roosyUrl;
  }
  const hyperclovaSlot = normalizeSlot(raw.hyperclovaSlot);
  const roosySlot = normalizeSlot(raw.roosySlot);
  return {
    hyperclovaUrl,
    roosyUrl,
    cachePrompt: raw.cachePrompt !== false,
    hyperclovaSlot: isManagedLoopbackCompletion(hyperclovaUrl) ? 0 : hyperclovaSlot,
    roosySlot: isManagedLoopbackCompletion(roosyUrl) ? 0 : roosySlot,
    startupTimeoutMs: Math.max(90000, normalizeMs(raw.startupTimeoutMs, 90000)),
    requestTimeoutMs: Math.max(240000, normalizeMs(raw.requestTimeoutMs, 240000)),
  };
};

const getStrategyChatModel = () => {
  const next = normalizeStrategyModel((ui as any).strategyChatModel);
  (ui as any).strategyChatModel = next;
  return next;
};

const closeStrategyChatModelMenu = () => {
  if (!(ui as any).strategyChatModelMenuOpen) return;
  (ui as any).strategyChatModelMenuOpen = false;
};

const appendStrategyChatProgress = (stage: string, content: string, rerender = true) => {
  const safeStage = String(stage || '').trim();
  const safeContent = String(content || '').trim();
  if (!safeContent) return;
  const line = safeStage ? `${safeStage} · ${safeContent}` : safeContent;
  const lines = getStrategyProgressLines();
  if (lines[lines.length - 1] !== line) lines.push(line);
  while (lines.length > STRATEGY_PROGRESS_MAX) lines.shift();
  (ui as any).strategyChatProgressStage = safeStage;
  log('strategy progress', line);
  if (rerender && ((ui as any).strategyChatPending || safeStage === 'error')) render();
};

const ensureStrategyChatProgressListener = () => {
  if (_strategyProgressListenerBound) return;
  _strategyProgressListenerBound = true;
  void listen<StrategyChatProgressPayload>('strategy-chat-progress', (event) => {
    const payload = event.payload || {};
    appendStrategyChatProgress(String(payload.stage || ''), String(payload.message || ''));
  }).catch((err) => {
    _strategyProgressListenerBound = false;
    log('strategy progress listener failed', err);
  });
};

const appendStrategyChatMessage = (
  role: StrategyChatRole,
  content: string,
  meta = '',
  extras: Partial<Pick<StrategyChatMessage, 'mode' | 'recordDraft' | 'sourceInput' | 'relatedRecordIds' | 'relatedCollectionTitle'>> = {},
) => {
  const safeContent = String(content || '').trim();
  if (!safeContent) return null;
  const msg: StrategyChatMessage = {
    id: uid('stratmsg'),
    role,
    content: safeContent,
    ts: nowISO(),
    meta: String(meta || '').trim(),
    mode: extras.mode === 'record' ? 'record' : 'analysis',
    ...(extras.recordDraft ? { recordDraft: true } : {}),
    ...(extras.sourceInput ? { sourceInput: String(extras.sourceInput || '') } : {}),
    ...(Array.isArray(extras.relatedRecordIds) ? { relatedRecordIds: extras.relatedRecordIds.map((id) => String(id || '').trim()).filter(Boolean) } : {}),
    ...(extras.relatedCollectionTitle ? { relatedCollectionTitle: String(extras.relatedCollectionTitle || '') } : {}),
  };
  getStrategyChatMessages().push(msg);
  syncActiveStrategyThreadPackage();
  return msg;
};

const updateStrategyChatMessage = (messageId: string, updater: (msg: StrategyChatMessage) => StrategyChatMessage) => {
  const messages = getStrategyChatMessages();
  const idx = messages.findIndex((item) => String(item.id || '') === String(messageId || '').trim());
  if (idx < 0) return null;
  const next = updater({ ...messages[idx] });
  messages[idx] = next;
  syncActiveStrategyThreadPackage();
  return next;
};

const clearStrategyChat = () => {
  (ui as any).strategyChatMessages = [];
  (ui as any).strategyChatInput = '';
  (ui as any).strategyChatCaret = -1;
  (ui as any).strategyChatError = '';
  (ui as any).strategyRecordEditMessageId = '';
  (ui as any).strategyRecordEditDraft = '';
  closeStrategyChatModelMenu();
  closeStrategyMentionMenu();
  (ui as any).strategyChatPending = false;
  (ui as any).strategyChatPendingStartedAt = '';
  (ui as any).strategyChatEvidencePacket = null;
  (ui as any).strategyChatRetrievalQuery = '';
  clearStrategyChatProgress();
};

let _strategyModelDownloadListenerBound = false;

const getStrategyModelStatus = () => (((ui as any).strategyModelStatus || null) as StrategyModelStatus | null);

const ensureStrategyModelDownloadListener = () => {
  if (_strategyModelDownloadListenerBound || !hasTauriWindow() || !isWindowsDesktop()) return;
  _strategyModelDownloadListenerBound = true;
  listen<StrategyModelDownloadProgress>('strategy-model-download-progress', (event) => {
    const payload = event.payload;
    if (!payload) return;
    (ui as any).strategyModelDownloadMessage = String(payload.message || '').trim();
    (ui as any).strategyModelDownloadLabel = String(payload.label || '').trim();
    (ui as any).strategyModelDownloadPercent = Number(payload.percent || 0);
    (ui as any).strategyModelDownloadIndeterminate = !!payload.indeterminate;
    (ui as any).strategyModelDownloadReceivedMb = Number(payload.downloadedBytes || 0) / (1024 * 1024);
    (ui as any).strategyModelDownloadTotalMb = Number(payload.totalBytes || 0) / (1024 * 1024);
    const line = `${String(payload.label || '모델')} · ${String(payload.message || '').trim() || '다운로드 진행 중'}`;
    appendStrategyChatProgress('모델다운로드', line, false);
    render();
  }).catch((err) => {
    _strategyModelDownloadListenerBound = false;
    log('strategy model download listener failed', err);
  });
};

const refreshStrategyModelStatus = async (opts?: { silent?: boolean }) => {
  if (!hasTauriWindow() || !isWindowsDesktop()) {
    (ui as any).strategyModelStatus = null;
    (ui as any).strategyModelStatusLoading = false;
    return null;
  }
  ensureStrategyModelDownloadListener();
  (ui as any).strategyModelStatusLoading = true;
  if (!opts?.silent) render();
  try {
    const status = await invoke('strategy_model_status') as StrategyModelStatus;
    (ui as any).strategyModelStatus = status;
    return status;
  } catch (err) {
    log('strategy model status failed', err);
    return null;
  } finally {
    (ui as any).strategyModelStatusLoading = false;
    if (!opts?.silent) render();
  }
};

let strategyModelStatusPollTimer: number | null = null;

const stopStrategyModelStatusPolling = () => {
  if (strategyModelStatusPollTimer != null) {
    window.clearInterval(strategyModelStatusPollTimer);
    strategyModelStatusPollTimer = null;
  }
};

const startStrategyModelStatusPolling = () => {
  stopStrategyModelStatusPolling();
  strategyModelStatusPollTimer = window.setInterval(async () => {
    try {
      const status = await refreshStrategyModelStatus({ silent: true });
      if (!status) return;
      if (status.allReady) {
        stopStrategyModelStatusPolling();
        (ui as any).strategyModelDownloadPending = false;
        (ui as any).strategyModelDownloadMessage = '모델 다운로드가 끝났어요. 이제 바로 채팅할 수 있어요.';
        (ui as any).strategyModelDownloadLabel = 'AI 모델';
        (ui as any).strategyModelDownloadIndeterminate = false;
        render();
      }
    } catch (_error) {
      // Keep polling quietly while the background download continues.
    }
  }, 2500);
};

const downloadStrategyModels = async () => {
  startStrategyModelStatusPolling();
  if ((ui as any).strategyModelDownloadPending) return;
  ensureStrategyModelDownloadListener();
  (ui as any).strategyModelDownloadPending = true;
  (ui as any).strategyModelDownloadMessage = 'HyperCLOVA-X와 Roosy-X를 내려받는 중이에요.';
  (ui as any).strategyModelDownloadLabel = 'ROOSY-Hybrid';
  (ui as any).strategyModelDownloadPercent = 0;
  (ui as any).strategyModelDownloadIndeterminate = true;
  (ui as any).strategyModelDownloadReceivedMb = 0;
  (ui as any).strategyModelDownloadTotalMb = 0;
  clearStrategyChatProgress();
  appendStrategyChatProgress('모델다운로드', 'AI 모델 두 개를 내려받는 중이에요.', false);
  render();
  try {
    const status = await invoke('download_strategy_models') as StrategyModelStatus;
    (ui as any).strategyModelStatus = status;
    (ui as any).strategyModelDownloadMessage = 'AI 모델 다운로드가 끝났어요.';
    (ui as any).strategyModelDownloadLabel = 'ROOSY-Hybrid';
    (ui as any).strategyModelDownloadPercent = 100;
    (ui as any).strategyModelDownloadIndeterminate = false;
    (ui as any).strategyChatError = '';
    toast('AI 모델 다운로드가 끝났어요');
  } catch (err) {
    const message = String((err as any)?.message || err || 'AI 모델 다운로드에 실패했어요.');
    (ui as any).strategyModelDownloadMessage = message;
    appendStrategyChatProgress('모델다운로드', message, false);
    toast('AI 모델 다운로드 실패');
    log('strategy model download failed', err);
  } finally {
    (ui as any).strategyModelDownloadPending = false;
    render();
  }
};

const getStrategySelectedRecords = (): RecordItem[] => {
  const selectedIds = reseedSimulationSelection(false);
  const allowed = new Set(selectedIds.map((id) => String(id || '').trim()).filter(Boolean));
  return getSimulationBaseRecords().filter((record: any) => allowed.has(String(record.id || ''))) as RecordItem[];
};

const normalizeStrategySearchText = (value: string) =>
  String(value || '')
    .toLowerCase()
    .replace(/\s+/g, ' ')
    .trim();

const normalizeStrategyActorName = (value: string) =>
  String(value || '')
    .replace(/[()[\]{}]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();

const stripStrategyActorParticles = (value: string) =>
  String(value || '')
    .replace(/(에게는|에게도|에게서|한테는|한테도|으로는|으로도|에서만|에서|에게|한테|께서|께는|께도|으로|로는|로도|와는|과는|이는|가는|은|는|이|가|을|를|와|과|도)$/u, '')
    .trim();

const strategyActorNameKey = (value: string) => normalizeStrategyActorName(value).toLowerCase();
const strategyActorCanonicalKey = (value: string) =>
  stripStrategyActorParticles(
    normalizeStrategyActorName(value)
    .replace(/^(당사자|상대방|관련자|참여자|학생|보호자|학부모|교사|선생님)\s*/u, '')
  )
    .replace(/\s+/g, '')
    .toLowerCase();
const strategyActorNamesMatch = (left: string, right: string) => {
  const leftKey = strategyActorNameKey(left);
  const rightKey = strategyActorNameKey(right);
  if (!leftKey || !rightKey) return false;
  if (leftKey === rightKey) return true;
  const leftCanonical = strategyActorCanonicalKey(left);
  const rightCanonical = strategyActorCanonicalKey(right);
  if (!leftCanonical || !rightCanonical) return false;
  if (leftCanonical === rightCanonical) return true;
  if (leftCanonical.length < 2 || rightCanonical.length < 2) return false;
  return leftCanonical.includes(rightCanonical) || rightCanonical.includes(leftCanonical);
};
const strategyActorRefKey = (actor: Partial<ActorRef> | null | undefined) =>
  [
    String(actor?.type || '').trim(),
    String(actor?.name || '').trim(),
    String((actor as any)?.groupId || '').trim(),
    String((actor as any)?.groupLabel || '').trim(),
  ].join('::').toLowerCase();

const getStrategyMentionDirectory = (): StrategyMentionSuggestion[] =>
  getRelationshipGroups().flatMap((group) =>
    (Array.isArray(group.members) ? group.members : [])
      .map((member) => ({
        groupId: String(group.id || '').trim(),
        memberId: String(member?.id || '').trim(),
        name: String(member?.name || '').trim(),
        groupLabel: String(group.title || '').trim(),
      }))
      .filter((item) => item.groupId && item.memberId && item.name)
  );

const STRATEGY_STRUCTURED_MENTION_RE = /(^|[\s(])@([0-9A-Za-z가-힣._-]{1,24})\(([^()\n]{1,40})\)/gu;
const STRATEGY_BARE_MENTION_RE = /(^|[\s(])@([0-9A-Za-z가-힣._-]{1,24})(?!\()/gu;
const STRATEGY_ACTION_MARK_RE = /#([^#\n][^#\n]{0,320})#/gu;

const stripStrategyMentionsForPrompt = (value: string) =>
  String(value || '')
    .replace(STRATEGY_STRUCTURED_MENTION_RE, (_all, prefix, name) => `${prefix}${name}`)
    .replace(STRATEGY_BARE_MENTION_RE, (_all, prefix, name) => `${prefix}${name}`);

const extractStrategyActionNotes = (value: string) => {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const match of String(value || '').matchAll(STRATEGY_ACTION_MARK_RE)) {
    const note = String(match[1] || '').replace(/\s+/g, ' ').trim();
    const key = note.toLowerCase();
    if (!note || seen.has(key)) continue;
    seen.add(key);
    out.push(note);
  }
  return out;
};

const stripStrategyInputForPrompt = (value: string) =>
  stripStrategyMentionsForPrompt(value)
    .replace(STRATEGY_ACTION_MARK_RE, (_all, note) => `${String(note || '').trim()}`)
    .replace(/\s+/g, ' ')
    .trim();

const extractStrategyMentionNames = (value: string) => {
  const out: string[] = [];
  const seen = new Set<string>();
  const push = (raw: string) => {
    const name = normalizeStrategyActorName(raw);
    const key = strategyActorNameKey(name);
    if (!name || seen.has(key)) return;
    seen.add(key);
    out.push(name);
  };
  for (const match of String(value || '').matchAll(STRATEGY_STRUCTURED_MENTION_RE)) {
    push(String(match[2] || ''));
  }
  for (const match of String(value || '').matchAll(STRATEGY_BARE_MENTION_RE)) {
    push(String(match[2] || ''));
  }
  return out;
};

const resolveStrategyMentionedActors = (value: string): ActorRef[] => {
  const directory = getStrategyMentionDirectory();
  const out: ActorRef[] = [];
  const seen = new Set<string>();
  const push = (actor: ActorRef | null) => {
    if (!actor) return;
    const key = strategyActorRefKey(actor);
    if (!key || seen.has(key)) return;
    seen.add(key);
    out.push(actor);
  };

  for (const match of String(value || '').matchAll(STRATEGY_STRUCTURED_MENTION_RE)) {
    const name = String(match[2] || '');
    const groupKey = normalizeStrategySearchText(String(match[3] || ''));
    const picked = directory.find((item) =>
      strategyActorNamesMatch(item.name, name)
      && normalizeStrategySearchText(item.groupLabel) === groupKey
    );
    if (picked) push(makeRelationshipActorRef(picked.groupId, picked.memberId));
  }

  for (const match of String(value || '').matchAll(STRATEGY_BARE_MENTION_RE)) {
    const name = String(match[2] || '');
    const exactMatches = directory.filter((item) => strategyActorNamesMatch(item.name, name));
    if (exactMatches.length === 1) {
      push(makeRelationshipActorRef(exactMatches[0].groupId, exactMatches[0].memberId));
    }
  }

  return out;
};

const findActiveStrategyMention = (value: string, caret: number) => {
  const safeValue = String(value || '');
  const safeCaret = Math.max(0, Math.min(Number.isFinite(caret) ? caret : safeValue.length, safeValue.length));
  const before = safeValue.slice(0, safeCaret);
  const atIndex = before.lastIndexOf('@');
  if (atIndex < 0) return null;
  const prefix = atIndex > 0 ? before.slice(atIndex - 1, atIndex) : ' ';
  if (prefix && !/[\s(]/.test(prefix)) return null;
  const segment = before.slice(atIndex, safeCaret);
  if (!segment.startsWith('@')) return null;
  if (/\s/.test(segment) || segment.includes('(') || segment.includes(')')) return null;
  const query = segment.slice(1);
  if (!/^[0-9A-Za-z가-힣._-]*$/u.test(query)) return null;
  return {
    start: atIndex,
    end: safeCaret,
    query: normalizeStrategyActorName(query),
  };
};

const getStrategyMentionSuggestions = (query: string): StrategyMentionSuggestion[] => {
  const normalizedQuery = normalizeStrategySearchText(query);
  const unique = new Map<string, StrategyMentionSuggestion>();
  for (const item of getStrategyMentionDirectory()) {
    const key = `${item.groupId}::${item.memberId}`;
    if (!normalizedQuery) {
      unique.set(key, item);
      continue;
    }
    const hay = [
      normalizeStrategySearchText(item.name),
      normalizeStrategySearchText(item.groupLabel),
      normalizeStrategySearchText(`${item.name} ${item.groupLabel}`),
    ];
    if (hay.some((value) => value.includes(normalizedQuery))) {
      unique.set(key, item);
    }
  }
  return Array.from(unique.values())
    .sort((a, b) => {
      const aStarts = normalizeStrategySearchText(a.name).startsWith(normalizedQuery) ? 1 : 0;
      const bStarts = normalizeStrategySearchText(b.name).startsWith(normalizedQuery) ? 1 : 0;
      if (bStarts !== aStarts) return bStarts - aStarts;
      const groupCmp = a.groupLabel.localeCompare(b.groupLabel, 'ko');
      if (groupCmp !== 0) return groupCmp;
      return a.name.localeCompare(b.name, 'ko');
    })
    .slice(0, 7);
};

const closeStrategyMentionMenu = () => {
  (ui as any).strategyMentionOpen = false;
  (ui as any).strategyMentionQuery = '';
  (ui as any).strategyMentionStart = -1;
  (ui as any).strategyMentionEnd = -1;
  (ui as any).strategyMentionSelectedIndex = 0;
  (ui as any).strategyMentionSuggestions = [];
};

const closeStrategyActionPrompt = () => {
  (ui as any).strategyActionPromptOpen = false;
  (ui as any).strategyActionDraft = '';
};

const openStrategyActionPrompt = (caret: number) => {
  (ui as any).strategyChatCaret = caret;
  closeStrategyMentionMenu();
  (ui as any).strategyActionPromptOpen = true;
  (ui as any).strategyActionDraft = '';
  render();
  focusStrategyActionComposer();
};

const applyStrategyActionPrompt = () => {
  const liveDraft = document.querySelector<HTMLInputElement>('[data-action="draft-strategy-action"]');
  if (liveDraft) {
    (ui as any).strategyActionDraft = String(liveDraft.value || '');
  }
  const note = String((ui as any).strategyActionDraft || '').replace(/\s+/g, ' ').trim();
  if (!note) {
    toast('대응 내용을 입력해 주세요');
    focusStrategyActionComposer();
    return;
  }
  const input = String((ui as any).strategyChatInput || '');
  const caret = Math.max(0, Math.min(Number((ui as any).strategyChatCaret || input.length), input.length));
  const token = `#${note}# `;
  (ui as any).strategyChatInput = `${input.slice(0, caret)}${token}${input.slice(caret)}`;
  (ui as any).strategyChatCaret = caret + token.length;
  closeStrategyActionPrompt();
  render();
  focusStrategyChatComposer();
};

const syncStrategyMentionState = (value: string, caret: number) => {
  const active = findActiveStrategyMention(value, caret);
  (ui as any).strategyChatCaret = caret;
  if (!active) {
    closeStrategyMentionMenu();
    return;
  }
  const suggestions = getStrategyMentionSuggestions(active.query);
  if (!suggestions.length) {
    closeStrategyMentionMenu();
    return;
  }
  (ui as any).strategyMentionOpen = true;
  (ui as any).strategyMentionQuery = active.query;
  (ui as any).strategyMentionStart = active.start;
  (ui as any).strategyMentionEnd = active.end;
  (ui as any).strategyMentionSuggestions = suggestions;
  const currentIndex = Number((ui as any).strategyMentionSelectedIndex || 0);
  (ui as any).strategyMentionSelectedIndex = Math.max(0, Math.min(currentIndex, suggestions.length - 1));
};

const applyStrategyMentionSuggestion = (suggestion: StrategyMentionSuggestion) => {
  const input = String((ui as any).strategyChatInput || '');
  const start = Number((ui as any).strategyMentionStart);
  const end = Number((ui as any).strategyMentionEnd);
  if (start < 0 || end < start) return;
  const mentionText = `@${suggestion.name}(${suggestion.groupLabel}) `;
  const nextValue = `${input.slice(0, start)}${mentionText}${input.slice(end)}`;
  (ui as any).strategyChatInput = nextValue;
  (ui as any).strategyChatCaret = start + mentionText.length;
  closeStrategyMentionMenu();
  render();
  focusStrategyChatComposer();
};

const strategyMentionSuggestionsKey = (suggestions: StrategyMentionSuggestion[]) =>
  suggestions.map((item) => `${item.groupId}::${item.memberId}`).join('|');

const STRATEGY_STOP_NAMES = new Set(['오늘', '점심시간', '쉬는시간', '싸웠어', '싸움', '다툼', '문제', '상황']);

const extractStrategyPeopleFromInput = (value: string) => {
  const replaced = stripStrategyInputForPrompt(value)
    .replace(/이랑|랑|하고| 및 /g, '|')
    .replace(/와\s+/g, '|')
    .replace(/과\s+/g, '|')
    .replace(/[()"'`]/g, ' ');
  const people: string[] = [];
  const seen = new Set<string>();
  for (const chunk of replaced.split('|')) {
    const candidate = String(chunk || '')
      .trim()
      .split(/\s+/)
      .map((part) => part.replace(/^[^0-9A-Za-z가-힣]+|[^0-9A-Za-z가-힣]+$/g, '').trim())
      .find(Boolean) || '';
    const normalized = stripStrategyActorParticles(candidate);
    const compact = normalizeStrategySearchText(normalized);
    if (!compact || STRATEGY_STOP_NAMES.has(compact)) continue;
    if (seen.has(compact)) continue;
    seen.add(compact);
    people.push(normalized);
    if (people.length >= 3) break;
  }
  return people;
};

const extractStrategyTimeHintFromInput = (value: string) => {
  const text = stripStrategyInputForPrompt(value);
  if (!text) return '';
  if (text.includes('오늘') && text.includes('점심시간')) return '오늘 점심시간';
  if (text.includes('오늘') && text.includes('쉬는시간')) return '오늘 쉬는시간';
  if (text.includes('오늘')) return '오늘';
  if (text.includes('점심시간')) return '점심시간';
  if (text.includes('쉬는시간')) return '쉬는시간';
  const explicit = text.match(/(\d{1,2}\s*시(?:\s*\d{1,2}\s*분)?(?:경)?)|(\d{1,2}:\d{2})/);
  return explicit ? explicit[0].replace(/\s+/g, ' ').trim() : '';
};

const extractStrategyPlaceHintFromInput = (value: string) => {
  const text = stripStrategyInputForPrompt(value);
  for (const place of ['교실', '복도', '운동장', '급식실', '강당', '화장실', '교문', '버스', '메신저', '온라인']) {
    if (text.includes(place)) return place;
  }
  if (text.includes('점심시간') || text.includes('쉬는시간')) return '교내 장소 미상';
  return '';
};

const tokenizeStrategySearchText = (value: string) =>
  Array.from(new Set(
    normalizeStrategySearchText(
      stripStrategyInputForPrompt(value)
        .replace(/이랑|랑|하고| 및 /g, ' ')
        .replace(/와\s+/g, ' ')
        .replace(/과\s+/g, ' ')
    )
      .replace(/[^\p{L}\p{N}\s]/gu, ' ')
      .split(/\s+/)
      .map((item) => item.trim())
      .filter((item) => item.length >= 2)
  ));

const buildStrategyRecordSearchBlob = (record: any) => {
  const actorBits = [
    ...(Array.isArray(record?.actors)
      ? record.actors.flatMap((actor: any) => [String(actor?.type || ''), String(actor?.name || '')])
      : []),
    ...(Array.isArray(record?.related)
      ? record.related.flatMap((actor: any) => [String(actor?.type || ''), String(actor?.name || '')])
      : []),
  ];
  const summary = [
    String(record?.summaryText || ''),
    String(record?.summary || ''),
    String(record?.title || ''),
    String(record?.note || ''),
    String(record?.memo || ''),
    String(record?.place || ''),
    String(record?.placeDetail || ''),
    String(record?.placeOther || ''),
    String(record?.storeType || ''),
    String(record?.channel || ''),
    ...actorBits,
  ].join(' ');
  const normalized = normalizeStrategySearchText(summary);
  return `${normalized} ${normalized.replace(/\s+/g, '')}`.trim();
};

const summarizeStrategyRecordForNote = (record: any) => {
  const actors = Array.isArray(record?.actors)
    ? record.actors
        .map((actor: any) => String(actor?.name || '').trim())
        .filter(Boolean)
        .slice(0, 2)
        .join(' · ')
    : '';
  const summary = String(record?.summaryText || record?.summary || record?.title || '').replace(/\s+/g, ' ').trim();
  return `${actors || '관련 인물 미상'} | ${summary.slice(0, 72) || '요약 없음'}`;
};

const mergeStrategyRecords = (...groups: RecordItem[][]) => {
  const seen = new Set<string>();
  const merged: RecordItem[] = [];
  for (const group of groups) {
    for (const record of group || []) {
      const id = String((record as any)?.id || '').trim();
      if (!id || seen.has(id)) continue;
      seen.add(id);
      merged.push(record);
    }
  }
  return merged;
};

const findStrategyRelatedRecords = (input: string, limit = 5): RecordItem[] => {
  const tokens = tokenizeStrategySearchText(input);
  if (!tokens.length) return [];
  const normalizedInput = normalizeStrategySearchText(stripStrategyInputForPrompt(input));
  const mentionedActors = resolveStrategyMentionedActors(input);
  const people = Array.from(new Set([
    ...mentionedActors.map((actor) => normalizeStrategyActorName(actor.name)),
    ...extractStrategyPeopleFromInput(input),
  ].filter(Boolean)));
  const placeHint = normalizeStrategySearchText(extractStrategyPlaceHintFromInput(input));
  const timeHint = normalizeStrategySearchText(extractStrategyTimeHintFromInput(input));
  const recs = Object.values(S.records || {})
    .map((record) => ensureRecordV8(record as any) as any)
    .filter(Boolean);

  return recs
    .map((record: any) => {
      const blob = buildStrategyRecordSearchBlob(record);
      const actorPool = [
        ...recordMainActors(record),
        ...(Array.isArray(record?.related) ? record.related : []),
      ].map((actor: any) => ({
        type: actor?.type || '기타',
        name: String(actor?.name || '').trim(),
        groupId: String(actor?.groupId || '').trim(),
        groupLabel: String(actor?.groupLabel || '').trim(),
      }));
      const explicitActorMatchCount = mentionedActors.filter((actor) =>
        actorPool.some((candidate) =>
          strategyActorRefKey(candidate) === strategyActorRefKey(actor)
          || (
            strategyActorNamesMatch(candidate.name, actor.name)
            && (!String(actor.groupLabel || '').trim()
              || normalizeStrategySearchText(candidate.groupLabel) === normalizeStrategySearchText(String(actor.groupLabel || '')))
          )
        )
      ).length;
      const actorNameMatchCount = people.filter((name) =>
        actorPool.some((candidate) => strategyActorNamesMatch(candidate.name, name))
      ).length;
      const actorMatchCount = mentionedActors.length ? explicitActorMatchCount : actorNameMatchCount;
      let score = 0;
      let matched = 0;
      for (const token of tokens) {
        if (!blob.includes(token)) continue;
        matched += 1;
        score += token.length >= 4 ? 3 : 2;
      }
      if (matched >= 2) score += 2;
      if (actorMatchCount > 0) score += actorMatchCount * (mentionedActors.length ? 7 : 5);
      if (people.length >= 2 && actorMatchCount >= 2) score += 4;
      if (mentionedActors.length > 0 && actorMatchCount === 0) score -= 6;
      else if (people.length > 0 && actorMatchCount === 0) score -= 3;
      if (placeHint && blob.includes(placeHint)) score += 2;
      if (timeHint && blob.includes(timeHint)) score += 1;
      if (/(싸웠|다퉜|갈등|언쟁|민원|폭행|욕설|울음|분리)/.test(normalizedInput) && /(싸움|갈등|언쟁|민원|폭행|욕설|울음|분리)/.test(blob)) {
        score += 2;
      }
      return { record, score, actorMatchCount };
    })
    .filter((item) => {
      if (!people.length) return item.score >= 2;
      if (item.actorMatchCount > 0) return item.score >= 1;
      return item.score >= 6;
    })
    .sort((a, b) => {
      if (b.actorMatchCount !== a.actorMatchCount) return b.actorMatchCount - a.actorMatchCount;
      if (b.score !== a.score) return b.score - a.score;
      return String((b.record as any)?.ts || '').localeCompare(String((a.record as any)?.ts || ''));
    })
    .slice(0, limit)
    .map((item) => item.record as RecordItem);
};

const findStrategyCollectionSuggestionRecords = (input: string, limit = 8): RecordItem[] => {
  const mentionedActors = resolveStrategyMentionedActors(input);
  const people = Array.from(new Set([
    ...mentionedActors.map((actor) => normalizeStrategyActorName(actor.name)),
    ...extractStrategyPeopleFromInput(input),
    ...extractStrategyMentionNames(input),
  ].filter(Boolean)));
  if (!people.length && !mentionedActors.length) return [];

  const recs = Object.values(S.records || {})
    .map((record) => ensureRecordV8(record as any) as any)
    .filter(Boolean);

  return recs
    .map((record: any) => {
      const actorPool = [
        ...recordMainActors(record),
        ...(Array.isArray(record?.related) ? record.related : []),
      ].map((actor: any) => ({
        type: actor?.type || '기타',
        name: String(actor?.name || '').trim(),
        groupId: String(actor?.groupId || '').trim(),
        groupLabel: String(actor?.groupLabel || '').trim(),
      }));
      const explicitActorMatchCount = mentionedActors.filter((actor) =>
        actorPool.some((candidate) =>
          strategyActorRefKey(candidate) === strategyActorRefKey(actor)
          || (
            strategyActorNamesMatch(candidate.name, actor.name)
            && (!String(actor.groupLabel || '').trim()
              || normalizeStrategySearchText(candidate.groupLabel) === normalizeStrategySearchText(String(actor.groupLabel || '')))
          )
        )
      ).length;
      const actorNameMatchCount = people.filter((name) =>
        actorPool.some((candidate) => strategyActorNamesMatch(candidate.name, name))
      ).length;
      const actorMatchCount = mentionedActors.length ? explicitActorMatchCount : actorNameMatchCount;
      const blob = buildStrategyRecordSearchBlob(record);
      const keywordBonus = people.filter((name) => {
        const compact = strategyActorCanonicalKey(name);
        return compact && blob.includes(compact);
      }).length;
      return { record, actorMatchCount, keywordBonus };
    })
    .filter((item) => item.actorMatchCount > 0 || item.keywordBonus > 0)
    .sort((a, b) => {
      if (b.actorMatchCount !== a.actorMatchCount) return b.actorMatchCount - a.actorMatchCount;
      if (b.keywordBonus !== a.keywordBonus) return b.keywordBonus - a.keywordBonus;
      return String((b.record as any)?.ts || '').localeCompare(String((a.record as any)?.ts || ''));
    })
    .slice(0, limit)
    .map((item) => item.record as RecordItem);
};

const buildStrategySuggestedCollectionTitle = (sourceInput: string, relatedRecords: RecordItem[]) => {
  const trimmed = stripStrategyInputForPrompt(sourceInput).replace(/\s+/g, ' ').trim();
  if (trimmed) {
    return `${trimmed.slice(0, 28)} 컬렉션`;
  }
  const first = relatedRecords[0] as any;
  const base = String(first?.summaryText || first?.summary || first?.title || '관련 기록').replace(/\s+/g, ' ').trim();
  return `${base.slice(0, 24) || '관련 기록'} 컬렉션`;
};

const isStrategyBroadActorName = (value: string) => {
  const normalized = String(value || '').replace(/\s+/g, ' ').trim();
  if (!normalized) return true;
  if (normalized.length <= 1) return true;
  if (/^[A-Za-z가-힣]$/.test(normalized)) return true;
  if (/^(학생|당사자|관련자|보호자|학부모|교사|상대방)\s*[A-Z가-힣0-9]?$/.test(normalized)) return true;
  if (/^(당사자\s*)?(학생|관련자|보호자|학부모|교사)\s*[A-Z]$/i.test(normalized)) return true;
  return false;
};

const buildStrategySuggestedActors = (records: RecordItem[], sourceInput = '') => {
  const seen = new Set<string>();
  const actors: ActorRef[] = [];
  const mentionedActors = resolveStrategyMentionedActors(sourceInput);
  const preferredNames = extractStrategyPeopleFromInput(sourceInput);
  const allActors = records.flatMap((record: any) => recordMainActors(record));

  for (const actor of mentionedActors) {
    const key = strategyActorRefKey(actor);
    if (seen.has(key)) continue;
    seen.add(key);
    actors.push({
      type: actor.type || '기타',
      name: String(actor.name || '').trim(),
      ...(String((actor as any)?.groupId || '').trim() ? { groupId: String((actor as any).groupId).trim() } : {}),
      ...(String((actor as any)?.groupLabel || '').trim() ? { groupLabel: String((actor as any).groupLabel).trim() } : {}),
    });
  }

  for (const preferredName of preferredNames) {
    const matched = allActors.find((actor: any) => strategyActorNamesMatch(String(actor?.name || ''), preferredName));
    if (!matched) continue;
    const name = String(matched?.name || '').trim();
    if (!name || isStrategyBroadActorName(name)) continue;
    const key = strategyActorRefKey(matched);
    if (seen.has(key)) continue;
    seen.add(key);
    actors.push({
      type: matched?.type || '기타',
      name,
      ...(String((matched as any)?.groupId || '').trim() ? { groupId: String((matched as any).groupId).trim() } : {}),
      ...(String((matched as any)?.groupLabel || '').trim() ? { groupLabel: String((matched as any).groupLabel).trim() } : {}),
    });
  }

  const frequency = new Map<string, { actor: any; count: number }>();
  for (const actor of allActors) {
    const name = String(actor?.name || '').trim();
    if (!name || isStrategyBroadActorName(name)) continue;
    const key = strategyActorRefKey(actor);
    const current = frequency.get(key);
    if (current) current.count += 1;
    else frequency.set(key, { actor, count: 1 });
  }

  const sortedFrequentActors = Array.from(frequency.values())
    .sort((a, b) => {
      if (b.count !== a.count) return b.count - a.count;
      return String(a.actor?.name || '').localeCompare(String(b.actor?.name || ''));
    })
    .map((item) => item.actor);

  for (const actor of sortedFrequentActors) {
    if (actors.length >= 6) break;
      const name = String(actor?.name || '').trim();
      if (!name || isStrategyBroadActorName(name)) continue;
      const key = strategyActorRefKey(actor);
      if (seen.has(key)) continue;
      seen.add(key);
      actors.push({
        type: actor?.type || '기타',
        name,
        ...(String(actor?.groupId || '').trim() ? { groupId: String(actor.groupId).trim() } : {}),
        ...(String(actor?.groupLabel || '').trim() ? { groupLabel: String(actor.groupLabel).trim() } : {}),
      });
  }
  return actors;
};

const buildStrategyNote = (contextRecords: RecordItem[] = [], mode: StrategyChatMode = getStrategyChatMode(), sourceInput = '') => {
  const draft = ensureSimulationDraft();
  const result = ((ui as any).simulationResult || null) as any;
  const selectedCaseId = String((ui as any).simulationCaseId || '').trim();
  const selectedCase = selectedCaseId && S.cases[selectedCaseId] ? S.cases[selectedCaseId] : null;
  const mentionedActors = resolveStrategyMentionedActors(sourceInput);
  const actionNotes = extractStrategyActionNotes(sourceInput);
  const presetLabel = draft.scenarioPreset === 'shield' ? '완화형' : draft.scenarioPreset === 'assertive' ? '단호형' : '균형형';
  const goalLabel = draft.goal === 'document' ? '기록 축적' : draft.goal === 'escalate' ? '공유·제출 강화' : '상황 안정';
  const lines = [
    `현재 목표: ${goalLabel}`,
    `전략 프리셋: ${presetLabel}`,
    `AI에게 반영할 메모: ${String(draft.evidenceFilter || '').trim() || '없음'}`,
    selectedCase ? `기준 컬렉션: ${String(selectedCase.title || '').trim() || '제목 없는 컬렉션'}` : '기준 컬렉션: 직접 분석 모드',
  ];
  if (result) {
    lines.push(
      `로컬 브리핑 지표: 근거 ${Number(result.evidencePower || 0)}, 논리 ${Number(result.counterLogic || 0)}, 확산 ${Number(result.escalationRisk || 0)}, 통제 ${Number(result.communicationControl || 0)}`,
      `추천 톤: ${String(result.recommendedTone || '').trim() || '미계산'}`,
      `추천 행동: ${String(result.recommendedAction || '').trim() || '미계산'}`,
    );
  }
  if (contextRecords.length) {
    lines.push(`참고 기록: ${contextRecords.length}개`);
    contextRecords.slice(0, 3).forEach((record) => {
      lines.push(`- ${summarizeStrategyRecordForNote(record)}`);
    });
  }
  if (mentionedActors.length) {
    lines.push(`관계관리에서 지정한 주체: ${mentionedActors.map((actor) => actorShort(actor)).join(', ')}`);
  }
  if (actionNotes.length) {
    lines.push(`사용자가 #로 표시한 직접 조치: ${actionNotes.join(' / ')}`);
    lines.push('직접 조치는 [내 대응 메모]에 우선 반영하고, 시간 흐름과 분리해서 적어주세요.');
  }
  if (mode === 'record') {
    lines.push('현재 요청은 통합 기록 생성 흐름으로 처리합니다. 관련 기록이 있으면 기록 초안에 함께 반영합니다.');
  }
  return lines.join('\n');
};

const sendStrategyAgentMessage = async (overrideMessage?: string) => {
  const selectedModel = getStrategyChatModel();
  if (hasTauriWindow() && isWindowsDesktop()) {
    const status = getStrategyModelStatus() || await refreshStrategyModelStatus({ silent: true });
    if (!status?.allReady) {
      (ui as any).strategyChatError = '우선 AI모델을 다운로드 받아주세요.';
      render();
      toast('우선 AI모델을 다운로드 받아주세요');
      return;
    }
  }
  const selectedRecords = getStrategySelectedRecords();
  const rawInput = String((overrideMessage ?? (ui as any).strategyChatInput) || '').trim();
  const relatedRecords = rawInput ? findStrategyRelatedRecords(rawInput, 6) : [];
  const collectionCandidateRecords = rawInput ? findStrategyCollectionSuggestionRecords(rawInput, 10) : [];
  const records = mergeStrategyRecords(selectedRecords, relatedRecords);
  const mode = getStrategyChatMode();
  (ui as any).strategyChatMode = mode;

  if (mode === 'record' && !rawInput) {
    toast('상황을 입력하면 기록 초안을 만들어드릴게요');
    return;
  }
  const message = mode === 'record' ? stripStrategyInputForPrompt(rawInput) : (stripStrategyInputForPrompt(rawInput) || STRATEGY_DEFAULT_PROMPT);
  const selectedCaseId = String((ui as any).simulationCaseId || '').trim();
  const caseItem = selectedCaseId && S.cases[selectedCaseId] ? S.cases[selectedCaseId] as CaseItem : null;
  const currentCaseRecordIds = new Set(
    Array.isArray(caseItem?.recordIds)
      ? (caseItem!.recordIds as string[]).map((id) => String(id || '').trim()).filter(Boolean)
      : []
  );
  const suggestedCollectionRecords = mergeStrategyRecords(relatedRecords, collectionCandidateRecords).filter((record) => {
    const recordId = String((record as any)?.id || '').trim();
    if (!recordId) return false;
    if (!currentCaseRecordIds.size) return true;
    return !currentCaseRecordIds.has(recordId);
  });
  const history = getStrategyChatMessages().slice(-8).map((item) => ({
    role: item.role,
    content: String(item.content || '').trim(),
  })).filter((item) => item.content);

  appendStrategyChatMessage('user', message, '', { mode });
  (ui as any).strategyChatInput = '';
  (ui as any).strategyChatCaret = -1;
  (ui as any).strategyChatError = '';
  (ui as any).strategyRecordEditMessageId = '';
  (ui as any).strategyRecordEditDraft = '';
  closeStrategyMentionMenu();
  closeStrategyChatModelMenu();
  (ui as any).strategyChatPending = true;
  (ui as any).strategyChatPendingStartedAt = nowISO();
  (ui as any).strategyPerfRunState = {
    status: 'running',
    requestedBackend: getStrategyBackendMode(),
    requestedMode: (ui as any).strategySyntheticCacheEnabled !== false ? 'FullDRACE' : 'Off',
    actualBackend: '',
    startedAt: nowISO(),
    finishedAt: '',
    error: '',
    cacheApplied: false,
  } as StrategyPerfRunState;
  clearStrategyChatProgress();
  appendStrategyChatProgress('준비', mode === 'record' ? 'AI 민원 법무팀 에이전트 기록 초안 요청을 접수했어요.' : 'AI 민원 법무팀 에이전트 요청을 접수했어요.', false);
  render();

  try {
    const maxTokens = 920;
    const shouldSuggestCollection = suggestedCollectionRecords.length >= 2;
    const result = await invoke('strategy_agent_chat', {
      args: {
        caseItem,
        records: records.map((record) => ensureRecordV8(record) as any),
        message,
        mode,
        model: selectedModel,
        strategyNote: buildStrategyNote(records, mode, rawInput),
        conversation: history,
        maxTokens,
        syntheticCacheEnabled: !!(ui as any).strategySyntheticCacheEnabled,
        backendMode: getStrategyBackendMode(),
        llamaServer: getStrategyBackendMode() === 'llama-server' ? getStrategyLlamaServerConfig() : null,
      },
    }) as StrategyChatInvokeResult;

    const answer = String(result?.answer || '').trim();
    if (!answer) throw new Error('모델이 빈 응답을 반환했어요.');
    (ui as any).strategyChatEvidencePacket = result?.evidencePacket || null;
    (ui as any).strategyChatRetrievalQuery = String(result?.retrievalQuery || '').trim();
    recordStrategyPerfSample(result?.perfMetrics || null);
    (ui as any).strategyPerfRunState = {
      status: 'succeeded',
      requestedBackend: getStrategyBackendMode(),
      requestedMode: (ui as any).strategySyntheticCacheEnabled !== false ? 'FullDRACE' : 'Off',
      actualBackend: String(result?.perfMetrics?.backendKind || ''),
      startedAt: String((ui as any).strategyPerfRunState?.startedAt || ''),
      finishedAt: nowISO(),
      error: '',
      cacheApplied: !!result?.perfMetrics?.cacheSummary?.cacheApplied,
    } as StrategyPerfRunState;
    appendStrategyChatMessage(
      'assistant',
      answer,
      `AI 법무팀 · 통합 기록 생성 · 참고 ${String(result.recordsUsed || records.length)}개`,
      {
        mode: 'record',
        recordDraft: true,
        sourceInput: rawInput,
        relatedRecordIds: shouldSuggestCollection
          ? suggestedCollectionRecords.map((record) => String((record as any)?.id || '').trim()).filter(Boolean)
          : [],
        relatedCollectionTitle: shouldSuggestCollection ? buildStrategySuggestedCollectionTitle(rawInput, suggestedCollectionRecords) : '',
      }
    );
    toast('기록 초안이 도착했어요');
  } catch (err) {
    const messageText = String((err as any)?.message || err || 'AI 민원 법무팀 에이전트 모델 호출에 실패했어요.');
    (ui as any).strategyChatError = messageText;
    (ui as any).strategyPerfRunState = {
      status: 'failed',
      requestedBackend: getStrategyBackendMode(),
      requestedMode: (ui as any).strategySyntheticCacheEnabled !== false ? 'FullDRACE' : 'Off',
      actualBackend: '',
      startedAt: String((ui as any).strategyPerfRunState?.startedAt || ''),
      finishedAt: nowISO(),
      error: messageText,
      cacheApplied: false,
    } as StrategyPerfRunState;
    appendStrategyChatProgress('오류', messageText, false);
    appendStrategyChatMessage('assistant', `AI 민원 법무팀 에이전트를 실행하지 못했어요.\n${messageText}`, '실행 오류');
    toast('AI 민원 법무팀 에이전트 실행 실패');
    log('strategy chat failed', err);
  } finally {
    (ui as any).strategyChatPending = false;
    (ui as any).strategyChatPendingStartedAt = '';
    closeStrategyMentionMenu();
    render();
  }
};

const openStrategyRecordDraftEditor = (messageId: string) => {
  const message = getStrategyChatMessages().find((item) => String(item.id || '') === String(messageId || '').trim());
  if (!message) return;
  (ui as any).strategyRecordEditMessageId = message.id;
  (ui as any).strategyRecordEditDraft = String(message.content || '');
  render();
};

const closeStrategyRecordDraftEditor = () => {
  (ui as any).strategyRecordEditMessageId = '';
  (ui as any).strategyRecordEditDraft = '';
};

const saveStrategyRecordDraftEdit = (messageId: string) => {
  const nextContent = String((ui as any).strategyRecordEditDraft || '').trim();
  if (!nextContent) return toast('수정한 기록 초안 내용을 입력해주세요');
  updateStrategyChatMessage(messageId, (msg) => ({
    ...msg,
    content: nextContent,
    mode: 'record',
    recordDraft: true,
  }));
  closeStrategyRecordDraftEditor();
  render();
  toast('기록 초안을 반영했어요');
};

const saveStrategyRecordDraftToArchive = (messageId: string) => {
  const message = getStrategyChatMessages().find((item) => String(item.id || '') === String(messageId || '') && item.role === 'assistant');
  if (!message) return toast('저장할 기록 초안을 찾지 못했어요');
  const raw = String((ui as any).strategyRecordEditMessageId === messageId ? (ui as any).strategyRecordEditDraft : message.content || '').trim();
  if (!raw) return toast('저장할 기록 초안이 비어 있어요');

  const allRecords = Object.values(S.records || {}).map((record: any) => ensureRecordV8(record as any) as RecordItem);
  const contextRecords = (Array.isArray((message as any).relatedRecordIds) ? (message as any).relatedRecordIds : [])
    .map((id: string) => allRecords.find((record) => String((record as any)?.id || '').trim() === String(id || '').trim()))
    .filter(Boolean) as RecordItem[];
  const { summaryText } = applyStrategyRecordDraftToComposer(raw, {
    sourceInput: String((message as any).sourceInput || '').trim(),
    contextRecords,
  });
  if (summaryText.length < 4) {
    return toast('기록 내용이 충분하지 않아 저장할 초안을 다시 확인해주세요');
  }
  (draftRecord as any).signerLabel = String((draftRecord as any).signerLabel || '').trim() || '기기 봉인서명';
  (draftRecord as any).sealReason = String((draftRecord as any).sealReason || '').trim() || 'AI 기록모드 초안 저장';
  closeStrategyRecordDraftEditor();
  openSignatureModal('create');
  render();
};

const createStrategyCollectionFromSuggestion = async (messageId: string) => {
  const message = getStrategyChatMessages().find((item) => String(item.id || '') === String(messageId || '').trim() && item.role === 'assistant') as any;
  if (!message) return toast('컬렉션을 만들 기록 초안을 찾지 못했어요');
  const relatedIds = Array.isArray(message.relatedRecordIds)
    ? message.relatedRecordIds.map((id: string) => String(id || '').trim()).filter(Boolean)
    : [];
  const allRecords = Object.values(S.records || {}).map((record: any) => ensureRecordV8(record as any) as RecordItem);
  const relatedRecords = relatedIds
    .map((id: string) => allRecords.find((record: RecordItem) => String((record as any)?.id || '').trim() === String(id || '').trim()))
    .filter(Boolean)
    .map((record: any) => ensureRecordV8(record as any) as RecordItem);
  if (!relatedRecords.length) return toast('연결할 관련 기록을 찾지 못했어요');
  if (!(await openConfirm('관련 기록이 있어 컬렉션을 생성할 수 있어요. 지금 컬렉션을 만들까요?'))) return;

  const sourceInput = String(message.sourceInput || '').trim();
  const title = String(message.relatedCollectionTitle || '').trim() || buildStrategySuggestedCollectionTitle(sourceInput, relatedRecords);
  const actors = buildStrategySuggestedActors(relatedRecords, sourceInput);
  const makeId = () => `case-${(typeof crypto !== 'undefined' && 'randomUUID' in crypto) ? crypto.randomUUID() : Math.random().toString(36).slice(2)}`;
  const exactRecordIds = relatedRecords
    .map((record: RecordItem) => String((record as any)?.id || '').trim())
    .filter(Boolean);
  if (!exactRecordIds.length) return toast('컬렉션으로 묶을 관련 기록이 없어요');

  const nextCase: CaseItem = {
    id: makeId(),
    title,
    actors,
    onlyMainActor: actors.length === 1,
    sensFilter: 'any',
    status: '진행중',
    createdAt: nowISO(),
    steps: [],
    query: sourceInput,
    timeFrom: '',
    timeTo: '',
    maxResults: Math.max(exactRecordIds.length, 24),
    recordIds: exactRecordIds,
    scoreByRecordId: Object.fromEntries(exactRecordIds.map((id: string) => [id, 1])),
    componentsByRecordId: {},
  };
  nextCase.advisors = await regenerateCaseAdvisors(nextCase, allRecords);

  S.cases[nextCase.id] = nextCase;
  (ui as any).simulationCaseId = nextCase.id;
  (ui as any).simulationSelectedRecordIds = Array.isArray(nextCase.recordIds) ? nextCase.recordIds.slice(0, 12) : [];
  (ui as any).simulationDirty = true;
  await saveState(S);
  render();
  toast('관련 기록으로 컬렉션을 만들었어요');
};

let _boundStrategyCollectionSuggestionAction = false;
const bindStrategyCollectionSuggestionAction = () => {
  if (_boundStrategyCollectionSuggestionAction || typeof document === 'undefined') return;
  _boundStrategyCollectionSuggestionAction = true;
  document.addEventListener('click', async (event) => {
    const target = event.target as HTMLElement | null;
    const button = target?.closest?.('[data-action="create-strategy-record-collection"]') as HTMLElement | null;
    if (!button) return;
    event.preventDefault();
    const messageId = String(button.getAttribute('data-id') || '').trim();
    if (!messageId) return;
    await createStrategyCollectionFromSuggestion(messageId);
  });
};

bindStrategyCollectionSuggestionAction();

const autoResizeContentProofArea = (el: HTMLTextAreaElement | null) => {
  if (!el) return;
  el.style.height = 'auto';
  const next = Math.max(88, Math.min(el.scrollHeight || 0, 220));
  el.style.height = `${next}px`;
};

const autoResizeStrategyChatArea = (el: HTMLTextAreaElement | null) => {
  if (!el) return;
  el.style.height = 'auto';
  const next = Math.max(34, Math.min((el.scrollHeight || 0) + 4, 164));
  el.style.height = `${next}px`;
  syncStrategyChatComposerMirror(el);
};

let _boundStrategyChatDockedComposer = false;

const syncStrategyChatDockedComposer = () => {
  const host = document.querySelector<HTMLElement>('.serviceContent.serviceContentChatMode');
  const page = host?.querySelector<HTMLElement>('.strategyChatPage');
  const shell = host?.querySelector<HTMLElement>('.strategyChatOnlyShell');
  const thread = host?.querySelector<HTMLElement>('.strategyChatThreadOnly');
  const composer = host?.querySelector<HTMLElement>('.strategyChatOnlyComposer');

  if (!host || !page || !shell || !thread || !composer) return;

  const hostHeight = Math.floor(host.clientHeight || 0);
  if (hostHeight <= 0) return;

  host.style.overflow = 'hidden';
  page.style.height = `${hostHeight}px`;
  page.style.minHeight = `${hostHeight}px`;
  shell.style.height = `${hostHeight}px`;
  shell.style.minHeight = `${hostHeight}px`;

  composer.style.position = 'relative';
  composer.style.left = 'auto';
  composer.style.width = '100%';
  composer.style.right = 'auto';
  composer.style.bottom = 'auto';
  composer.style.margin = '0';
  composer.style.maxWidth = 'none';
  composer.style.transform = 'none';
  composer.style.zIndex = '1';

  const shellStyles = window.getComputedStyle(shell);
  const gap = parseFloat(shellStyles.rowGap || shellStyles.gap || '0') || 0;
  const paddingTop = parseFloat(shellStyles.paddingTop || '0') || 0;
  const paddingBottom = parseFloat(shellStyles.paddingBottom || '0') || 0;
  const composerHeight = Math.ceil(composer.getBoundingClientRect().height);
  const threadHeight = Math.max(180, hostHeight - paddingTop - paddingBottom - gap - composerHeight);
  const wasNearBottom = Math.max(0, thread.scrollHeight - thread.clientHeight - thread.scrollTop) <= 48;
  const fromBottom = Math.max(0, thread.scrollHeight - thread.clientHeight - thread.scrollTop);
  const prevTop = thread.scrollTop;

  thread.style.height = `${threadHeight}px`;
  thread.style.minHeight = `${threadHeight}px`;
  thread.style.maxHeight = `${threadHeight}px`;
  thread.style.overflowY = 'auto';
  thread.style.overflowX = 'hidden';
  thread.style.paddingBottom = '16px';
  thread.style.scrollPaddingBottom = '24px';

  const nextMaxScrollTop = Math.max(0, thread.scrollHeight - thread.clientHeight);
  if (wasNearBottom) {
    thread.scrollTop = nextMaxScrollTop;
  } else {
    thread.scrollTop = Math.max(0, nextMaxScrollTop - fromBottom);
  }
};

const queueStrategyChatDockedComposerSync = () => {
  window.requestAnimationFrame(() => {
    window.requestAnimationFrame(() => {
      syncStrategyChatDockedComposer();
    });
  });
};

const bindStrategyChatDockedComposer = () => {
  if (_boundStrategyChatDockedComposer) return;
  _boundStrategyChatDockedComposer = true;
  window.addEventListener('resize', () => {
    queueStrategyChatDockedComposerSync();
  });
};

const updateContentProofUI = () => {
  const draft = ensureContentProofDraft();
  const fieldMap: Record<string, string> = {
    senderName: String(draft.senderName || '').trim(),
    senderAddress: String(draft.senderAddress || '').trim(),
    recipientName: String(draft.recipientName || '').trim(),
    recipientAddress: String(draft.recipientAddress || '').trim(),
  };

  Object.entries(fieldMap).forEach(([field, value]) => {
    document.querySelectorAll<HTMLElement>(`[data-proof-bind="${field}"]`).forEach((node) => {
      const fallback = String(node.dataset.proofEmpty || '미입력');
      node.textContent = value || fallback;
      node.classList.toggle('isEmpty', !value);
    });

    const input = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[data-action="draft-content-proof"][data-field="${field}"]`);
    if (input && document.activeElement !== input && input.value !== String(draft[field as keyof typeof fieldMap] || '')) {
      input.value = String(draft[field as keyof typeof fieldMap] || '');
    }
    if (input instanceof HTMLTextAreaElement) autoResizeContentProofArea(input);
  });

  const senderReady = !!fieldMap.senderName && !!fieldMap.senderAddress;
  const recipientReady = !!fieldMap.recipientName && !!fieldMap.recipientAddress;
  const filledCount = Object.values(fieldMap).filter(Boolean).length;
  const allReady = senderReady && recipientReady;

  document.querySelectorAll<HTMLElement>('[data-proof-party="sender"]').forEach((node) => node.classList.toggle('ready', senderReady));
  document.querySelectorAll<HTMLElement>('[data-proof-party="recipient"]').forEach((node) => node.classList.toggle('ready', recipientReady));

  const status = document.getElementById('contentProofStatus');
  if (status) {
    status.textContent = allReady ? '발신인·수신인 정보 입력 완료' : `송달 정보 ${filledCount}/4 입력`;
    status.classList.toggle('ready', allReady);
  }

  const printBtn = document.querySelector<HTMLButtonElement>('#paperModal [data-action="print-paper"]');
  if (printBtn) {
    printBtn.disabled = !allReady;
    printBtn.setAttribute('aria-disabled', allReady ? 'false' : 'true');
    if (!allReady) printBtn.setAttribute('title', '발신인/수신인 이름과 주소를 모두 입력하면 PDF를 저장할 수 있어요');
    else printBtn.removeAttribute('title');
  }
};

const ensureClassRosterDraft = () => {
  const raw = Array.isArray(ui.classRosterDraft) ? ui.classRosterDraft : [];
  ui.classRosterDraft = cloneRelationshipGroups(raw.length ? raw as any : getRelationshipGroups());
  const firstGroupId = String(ui.classRosterDraft[0]?.id || 'group-1');
  if (!ui.classRosterDraft.some((group: any) => String(group?.id || '') === String(ui.classRosterGroupId || ''))) {
    ui.classRosterGroupId = firstGroupId;
  }
  return ui.classRosterDraft;
};

const updateClassRosterCountUI = () => {
  const countEl = document.getElementById('classRosterFilledCount');
  if (!countEl) return;
  const filled = ensureClassRosterDraft().reduce((acc, group: any) => (
    acc + (Array.isArray(group?.members) ? group.members.filter((member: any) => String(member?.name || '').trim()).length : 0)
  ), 0);
  countEl.textContent = String(filled);
};

const syncClassRosterFieldDefaults = () => {
  const groups = getRelationshipGroups();
  const firstGroupId = String(groups[0]?.id || 'group-1');
  const actorMembers = (groups.find((group) => group.id === draftRecord.actorGroupId)?.members || []);
  const relatedMembers = (groups.find((group) => group.id === draftRecord.relGroupId)?.members || []);
  const recordEditActorMembers = (groups.find((group) => group.id === draftRecordEdit.actorGroupId)?.members || []);
  const recordEditRelatedMembers = (groups.find((group) => group.id === draftRecordEdit.relGroupId)?.members || []);

  if (!groups.some((group) => group.id === draftRecord.actorGroupId)) draftRecord.actorGroupId = firstGroupId;
  if (!groups.some((group) => group.id === draftRecord.relGroupId)) draftRecord.relGroupId = firstGroupId;
  if (!groups.some((group) => group.id === draftRecordEdit.actorGroupId)) (draftRecordEdit as any).actorGroupId = firstGroupId;
  if (!groups.some((group) => group.id === draftRecordEdit.relGroupId)) (draftRecordEdit as any).relGroupId = firstGroupId;

  if (!actorMembers.some((member: any) => member.id === draftRecord.actorMemberId)) draftRecord.actorMemberId = '';
  if (!relatedMembers.some((member: any) => member.id === draftRecord.relMemberId)) draftRecord.relMemberId = '';
  if (!recordEditActorMembers.some((member: any) => member.id === (draftRecordEdit as any).actorMemberId)) (draftRecordEdit as any).actorMemberId = '';
  if (!recordEditRelatedMembers.some((member: any) => member.id === (draftRecordEdit as any).relMemberId)) (draftRecordEdit as any).relMemberId = '';
};

const SUMMARY_PART_LABELS = [
  ['overview', '상황 요약'],
  ['background', '배경 흐름'],
  ['issues', '핵심 포인트'],
  ['evidenceList', '관련 자료'],
  ['teacherActions', '내 대응 메모'],
  ['other', '추가 메모'],
] as const;

type DraftRecordLike = typeof draftRecord;

type DraftActorTypeText = string;
const preserveActorTypeText = (selectedText: DraftActorTypeText, internalType: ActorRef['type']) => {
  const normalized = normalizeActorTypeTextUI(String(selectedText || '').trim());
  if (normalized === UI_CLASS_ACTOR_LABEL) return UI_CLASS_ACTOR_LABEL;
  return normalized || actorTypeTextFromInternal(internalType as any);
};

const didActorTypePickerChange = (prevText: string, nextText: string) =>
  normalizeActorTypeTextUI(String(prevText || '').trim()) !== normalizeActorTypeTextUI(String(nextText || '').trim());

function getDraftSummaryParts(draft: any) {
  return {
    overview: String(draft.summaryOverview || '').trim(),
    background: String(draft.summaryBackground || '').trim(),
    issues: String(draft.summaryIssues || '').trim(),
    evidenceList: String(draft.summaryEvidenceList || '').trim(),
    teacherActions: String(draft.summaryTeacherActions || '').trim(),
    other: String(draft.summaryOther || '').trim(),
  };
}

function buildSummaryFromDraftParts(draft: any) {
  const parts = getDraftSummaryParts(draft);
  const joined = SUMMARY_PART_LABELS
    .map(([key, label]) => {
      const value = String((parts as any)[key] || '').trim();
      return value ? `[${label}]\n${value}` : '';
    })
    .filter(Boolean)
    .join('\n\n');
  return { parts, text: joined };
}

const STRATEGY_RECORD_SECTION_MAP: Record<string, keyof ReturnType<typeof getDraftSummaryParts>> = {
  '상황 요약': 'overview',
  '배경 흐름': 'background',
  '핵심 포인트': 'issues',
  '관련 자료': 'evidenceList',
  '내 대응 메모': 'teacherActions',
  '추가 메모': 'other',
};

type ParsedStrategyRecordDraft = {
  raw: string;
  metadata: {
    ts: string;
    actor: string;
    related: string;
    placeChannel: string;
    storeType: string;
    lv: string;
  };
  parts: ReturnType<typeof getDraftSummaryParts>;
};

function parseStrategyRecordDraftText(raw: string): ParsedStrategyRecordDraft {
  const text = String(raw || '').replace(/\r/g, '').trim();
  const sections = new Map<string, string[]>();
  let current = '';
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    const sectionMatch = trimmed.match(/^\[(.+?)\]\s*$/);
    if (sectionMatch) {
      current = sectionMatch[1].trim();
      if (!sections.has(current)) sections.set(current, []);
      continue;
    }
    if (!current) current = '상황 요약';
    if (!sections.has(current)) sections.set(current, []);
    sections.get(current)!.push(line);
  }

  const infoLines = (sections.get('기록 기본정보') || [])
    .map((line) => line.trim().replace(/^-+\s*/, ''))
    .filter(Boolean);
  const metadataMap = new Map<string, string>();
  infoLines.forEach((line) => {
    const match = line.match(/^([^:：]+)\s*[:：]\s*(.+)$/);
    if (!match) return;
    metadataMap.set(match[1].trim(), match[2].trim());
  });

  const readSection = (label: keyof typeof STRATEGY_RECORD_SECTION_MAP | '상황 요약') => {
    const lines = (sections.get(label) || []).join('\n').trim();
    return lines;
  };

  const fallbackOverview = text && !sections.size ? text : ((sections.get('상황 요약') || []).join('\n').trim() || text);

  return {
    raw: text,
    metadata: {
      ts: metadataMap.get('기록 시각') || metadataMap.get('시각') || '',
      actor: metadataMap.get('주체') || '',
      related: metadataMap.get('상대방') || '',
      placeChannel: metadataMap.get('위치/채널') || metadataMap.get('위치') || '',
      storeType: metadataMap.get('자료 형태') || metadataMap.get('보관형태') || metadataMap.get('채널') || '',
      lv: metadataMap.get('민감도') || '',
    },
    parts: {
      overview: fallbackOverview,
      background: readSection('배경 흐름'),
      issues: readSection('핵심 포인트'),
      evidenceList: readSection('관련 자료'),
      teacherActions: readSection('내 대응 메모'),
      other: readSection('추가 메모'),
    },
  };
}

const splitStrategyActorNames = (value: string) => String(value || '')
  .replace(/^(당사자|주체|상대방)\s*/g, '')
  .split(/\s*[·,\/|]\s*/)
  .map((item) => item.trim())
  .filter(Boolean);

const inferStrategyPlaceFromContextRecords = (records: RecordItem[]) => {
  const counts = new Map<string, number>();
  for (const record of records as any[]) {
    const place = String(record?.placeOther || '').trim() || String(record?.place || '').trim();
    if (!place || /미상/.test(place)) continue;
    counts.set(place, (counts.get(place) || 0) + 1);
  }
  return Array.from(counts.entries())
    .sort((a, b) => b[1] - a[1])
    .map(([place]) => place)[0] || '';
};

const normalizeStrategyDraftTs = (value: string) => {
  const text = String(value || '').trim();
  if (!text) return toLocalInputValue(nowISO());
  if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/.test(text)) return toLocalInputValue(text);
  const normalized = text.replace(/\./g, '-').replace(/\//g, '-').replace(/\s+/g, ' ').trim();
  const direct = normalized.match(/^(\d{4})-(\d{2})-(\d{2})\s+(\d{2}):(\d{2})$/);
  if (direct) return `${direct[1]}-${direct[2]}-${direct[3]}T${direct[4]}:${direct[5]}`;
  return toLocalInputValue(nowISO());
};

function getPendingDraftActor(draft: any): ActorRef | null {
  const groupedActor = makeRelationshipActorRef(String(draft.actorGroupId || ''), String(draft.actorMemberId || ''));
  if (groupedActor) return groupedActor;
  const typeText = String(draft.actorTypeText || '').trim();
  const name = String(draft.actorNameOther || '').trim();
  if (!typeText) return null;
  if (typeText === UI_OTHER_ACTOR_LABEL || typeText === '없음') return null;
  if (!name) return null;
  return { type: actorTypeInternalFromText(typeText) as any, name };
}

function getDraftActorsForSave(draft: any): ActorRef[] {
  const current = Array.isArray(draft.actors) ? draft.actors : [];
  let out = current
    .map((a: any) => ({ type: (a?.type ?? '외부인') as any, name: String(a?.name ?? '').trim() }))
    .filter((a: any) => a.name)
    .reduce((acc: ActorRef[], a: ActorRef) => addActorToList(acc, a), [] as ActorRef[]);
  const pending = getPendingDraftActor(draft);
  if (pending) out = addActorToList(out, pending);
  return out;
}

// 메모 입력폼(컴포저)에서 저장 버튼/필수 경고를 전체 리렌더 없이 즉시 반영
function updateRecordComposerUI() {
  const btn = document.getElementById('btnSaveRecord') as HTMLButtonElement | null;
  const pill = document.getElementById('recordReqPill') as HTMLSpanElement | null;
  const wSum = document.getElementById('recordWarnSummary') as HTMLDivElement | null;
  const wTs = document.getElementById('recordWarnTs') as HTMLDivElement | null;
  const wAct = document.getElementById('recordWarnActor') as HTMLDivElement | null;

  if (!btn && !pill && !wSum && !wTs && !wAct) return;

  const summaryPack = buildSummaryFromDraftParts(draftRecord as any);
  const okSummary = summaryPack.text.length >= 4;
  const okTs = String(draftRecord.ts || '').trim().length >= 10;
  const okActor = getDraftActorsForSave(draftRecord as any).length > 0;

  const reqMissing: string[] = [];
  if (!okSummary) reqMissing.push('내용');
  if (!okTs) reqMissing.push('시간');
  if (!okActor) reqMissing.push('주체');
  const canSave = okSummary && okTs && okActor;
  const reqLabel = canSave ? '필수 입력 완료' : `필수: ${reqMissing.join(' · ')}`;

  if (pill) {
    pill.textContent = reqLabel;
    pill.classList.toggle('ready', canSave);
    pill.classList.toggle('warn', !canSave);
  }

  if (btn) {
    btn.disabled = !canSave;
    btn.setAttribute('aria-disabled', canSave ? 'false' : 'true');
    if (!canSave) btn.setAttribute('title', '필수 항목(내용/시간/주체)을 채우면 저장할 수 있어요');
    else btn.removeAttribute('title');
  }

  if (wSum) (wSum as any).hidden = okSummary;
  if (wTs) (wTs as any).hidden = okTs;
  if (wAct) (wAct as any).hidden = okActor;

  const elTs = document.getElementById('recordTs') as HTMLInputElement | null;
  const elActorRow = document.getElementById('recordActorRow') as HTMLDivElement | null;
  const elSummaryWrap = document.getElementById('recordSummaryParts') as HTMLDivElement | null;
  if (elSummaryWrap) elSummaryWrap.classList.toggle('reqWarn', !okSummary);
  if (elTs) elTs.classList.toggle('reqWarn', !okTs);
  if (elActorRow) elActorRow.classList.toggle('reqWarn', !okActor);
}


function updateRecordEditUI() {
  const btn = document.getElementById('btnSaveRecordAmend') as HTMLButtonElement | null;
  const pill = document.getElementById('recordEditReqPill') as HTMLSpanElement | null;
  const wSum = document.getElementById('recordEditWarnSummary') as HTMLDivElement | null;
  const wTs = document.getElementById('recordEditWarnTs') as HTMLDivElement | null;
  const wAct = document.getElementById('recordEditWarnActor') as HTMLDivElement | null;
  if (!btn && !pill && !wSum && !wTs && !wAct) return;

  const summaryPack = buildSummaryFromDraftParts(draftRecordEdit as any);
  const okSummary = summaryPack.text.length >= 4;
  const okTs = String(draftRecordEdit.ts || '').trim().length >= 10;
  const okActor = getDraftActorsForSave(draftRecordEdit as any).length > 0;

  const reqMissing: string[] = [];
  if (!okSummary) reqMissing.push('내용');
  if (!okTs) reqMissing.push('기록시각');
  if (!okActor) reqMissing.push('주체');
  const canSave = okSummary && okTs && okActor;
  const reqLabel = canSave ? '정정 봉인 가능' : `필수: ${reqMissing.join(' · ')}`;

  if (pill) {
    pill.textContent = reqLabel;
    pill.classList.toggle('ready', canSave);
    pill.classList.toggle('warn', !canSave);
  }
  if (btn) {
    btn.disabled = !canSave;
    btn.setAttribute('aria-disabled', canSave ? 'false' : 'true');
    if (!canSave) btn.setAttribute('title', '내용/기록시각/사람을 채우면 정정 봉인할 수 있어요');
    else btn.removeAttribute('title');
  }
  if (wSum) (wSum as any).hidden = okSummary;
  if (wTs) (wTs as any).hidden = okTs;
  if (wAct) (wAct as any).hidden = okActor;

  const elTs = document.getElementById('recordEditTs') as HTMLInputElement | null;
  const elActorRow = document.getElementById('recordEditActorRow') as HTMLDivElement | null;
  const elSummaryWrap = document.getElementById('recordEditSummaryParts') as HTMLDivElement | null;
  if (elSummaryWrap) elSummaryWrap.classList.toggle('reqWarn', !okSummary);
  if (elTs) elTs.classList.toggle('reqWarn', !okTs);
  if (elActorRow) elActorRow.classList.toggle('reqWarn', !okActor);
}

// 렌더 전: render()가 DOM을 갈아엎기 때문에, <details> 같은 transient UI 상태를 저장해둔다.
function captureTransientUI() {
  const det = document.getElementById('recordRelatedDetails') as HTMLDetailsElement | null;
  if (det) ui.recRelatedOpen = !!det.open;
  const detEdit = document.getElementById('recordEditRelatedDetails') as HTMLDetailsElement | null;
  if (detEdit) ui.recEditRelatedOpen = !!detEdit.open;
}

type RenderScrollState = {
  winX: number;
  winY: number;
  containers: { selector: string; top: number; left: number; fromBottom?: number | null; preserveFromBottom?: boolean }[];
};

function captureRenderScrollState(): RenderScrollState {
  const selectors = [
    { selector: '#recordComposerModal' },
    { selector: '#recordComposerModal .recordComposerModalBody' },
    { selector: '#recordModal' },
    { selector: '#recordModal .modalBody' },
    { selector: '#caseCreateModal' },
    { selector: '#caseCreateModal .caseCommandPanelScroll' },
    { selector: '#caseUpdateModal' },
    { selector: '#timelineDetailModal' },
    { selector: '.strategyChatThreadOnly', preserveFromBottom: true },
  ];
  const containers = selectors
    .map((item) => {
      const el = document.querySelector(item.selector) as HTMLElement | null;
      if (!el) return null;
      const fromBottom = item.preserveFromBottom
        ? Math.max(0, el.scrollHeight - el.clientHeight - el.scrollTop)
        : null;
      return { selector: item.selector, top: el.scrollTop, left: el.scrollLeft, fromBottom, preserveFromBottom: !!item.preserveFromBottom };
    })
    .filter(Boolean) as { selector: string; top: number; left: number; fromBottom?: number | null; preserveFromBottom?: boolean }[];
  return {
    winX: window.scrollX || 0,
    winY: window.scrollY || 0,
    containers,
  };
}

function restoreRenderScrollState(state: RenderScrollState | null) {
  if (!state) return;
  window.scrollTo(state.winX, state.winY);
  for (const item of state.containers) {
    const el = document.querySelector(item.selector) as HTMLElement | null;
    if (!el) continue;
    if (item.preserveFromBottom) {
      const maxScrollTop = Math.max(0, el.scrollHeight - el.clientHeight);
      const fromBottom = Math.max(0, Number(item.fromBottom ?? 0));
      el.scrollTop = Math.max(0, maxScrollTop - fromBottom);
    } else {
      el.scrollTop = item.top;
    }
    el.scrollLeft = item.left;
  }
}

type FocusedSimulationFieldState = {
  field: 'evidenceFilter' | 'pickerQuery';
  start: number | null;
  end: number | null;
};

type FocusedStrategyChatFieldState = {
  start: number | null;
  end: number | null;
};

function captureFocusedSimulationFieldState(): FocusedSimulationFieldState | null {
  const active = document.activeElement as HTMLInputElement | null;
  if (!active) return null;
  if (active.dataset.action !== 'draft-simulation') return null;
  const field = String(active.dataset.field || '').trim();
  if (!(field === 'evidenceFilter' || field === 'pickerQuery')) return null;
  return {
    field: field as 'evidenceFilter' | 'pickerQuery',
    start: typeof active.selectionStart === 'number' ? active.selectionStart : null,
    end: typeof active.selectionEnd === 'number' ? active.selectionEnd : null,
  };
}

function restoreFocusedSimulationFieldState(state: FocusedSimulationFieldState | null) {
  if (!state) return;
  window.setTimeout(() => {
    const selector = `[data-action="draft-simulation"][data-field="${state.field}"]`;
    const el = document.querySelector(selector) as HTMLInputElement | null;
    if (!el) return;
    el.focus({ preventScroll: true });
    const valueLength = String(el.value || '').length;
    const start = state.start == null ? valueLength : Math.max(0, Math.min(state.start, valueLength));
    const end = state.end == null ? start : Math.max(start, Math.min(state.end, valueLength));
    try {
      el.setSelectionRange(start, end);
    } catch {}
  }, 0);
}

function captureFocusedStrategyChatFieldState(): FocusedStrategyChatFieldState | null {
  const active = document.activeElement as HTMLTextAreaElement | null;
  if (!active) return null;
  if (active.dataset.action !== 'draft-strategy-chat' || active.dataset.field !== 'input') return null;
  return {
    start: typeof active.selectionStart === 'number' ? active.selectionStart : null,
    end: typeof active.selectionEnd === 'number' ? active.selectionEnd : null,
  };
}

function restoreFocusedStrategyChatFieldState(state: FocusedStrategyChatFieldState | null) {
  if (!state) return;
  window.setTimeout(() => {
    const el = document.querySelector('[data-action="draft-strategy-chat"][data-field="input"]') as HTMLTextAreaElement | null;
    if (!el) return;
    el.focus({ preventScroll: true });
    const valueLength = String(el.value || '').length;
    const start = state.start == null ? valueLength : Math.max(0, Math.min(state.start, valueLength));
    const end = state.end == null ? start : Math.max(start, Math.min(state.end, valueLength));
    try {
      el.setSelectionRange(start, end);
    } catch {}
  }, 0);
}

const render = () => {
  captureTransientUI();
  const scrollState = captureRenderScrollState();
  const focusedSimulationFieldState = captureFocusedSimulationFieldState();
  const focusedStrategyChatFieldState = captureFocusedStrategyChatFieldState();
  _isRerendering = true;
  renderView();
  bindWindowDragRegionFallback();
  syncDialogs();
  restoreRenderScrollState(scrollState);
  restoreFocusedSimulationFieldState(focusedSimulationFieldState);
  restoreFocusedStrategyChatFieldState(focusedStrategyChatFieldState);
  updateRecordComposerUI();
  updateRecordEditUI();
  updateContentProofUI();
  autoResizeStrategyChatArea(document.querySelector<HTMLTextAreaElement>('[data-action="draft-strategy-chat"][data-field="input"]'));
  bindStrategyChatDockedComposer();
  queueStrategyChatDockedComposerSync();
  window.setTimeout(() => { _isRerendering = false; }, 0);
};

const SR = async () => { await saveState(S); render(); };
const toastUndo = (msg: string, undo: () => Promise<void>) => toast(msg, { label: '되돌리기', onClick: undo });
const flash = (id: string) => { ui.flashStepId = id; ui.flashStepTimer && clearTimeout(ui.flashStepTimer); ui.flashStepTimer = window.setTimeout(() => (ui.flashStepId = null, render()), 1800); };
const mustCase = (msg = '기록묶음을 먼저 선택하세요') => { const c = getSelectedCase(); if (!c) toast(msg); return c; };
const openUpdate = (caseId: string) => (
  ui.updateCaseId = caseId,
  ui.qUpdate = '',
  ui.updatePickIds = [],
  ui.updFilterActor = ui.updFilterPlace = ui.updFilterKeyword = '',
  ui.updFilterActorDraft = ui.updFilterPlaceDraft = ui.updFilterKeywordDraft = '',
  render(),
  openCaseUpdateModal(),
  void refreshUpdateCandidates(caseId)
);

/* ---------- sample pack (demo) ---------- */
const SAMPLE_PACK_URL = new URL('../ui/sample_pack_v7.json', import.meta.url);
async function loadSamplePackJSON(): Promise<any> {
  const res = await fetch(SAMPLE_PACK_URL, { cache: 'no-store' });
  if (!res.ok) throw new Error(`sample fetch failed: ${res.status}`);
  return await res.json();
}

/* ---------- case update candidates ---------- */
async function refreshUpdateCandidates(caseId: string) {
  const c = S.cases[caseId]; if (!c) return;
  ui.updateCandidatesLoading = true; ui.updateCandidatesForCaseId = caseId; ui.updateCandidates = null;
  render(); openCaseUpdateModal();
  try {
    const cand = await getCaseUpdateCandidates(c, S.records);
    if (ui.updateCaseId === caseId) ui.updateCandidates = cand;
  } catch (e) {
    ui.updateCandidates = []; log('update candidates failed', e);
  } finally {
    ui.updateCandidatesLoading = false; render(); openCaseUpdateModal();
  }
}


/* ---------- defaults (draft) ---------- */
const DEFAULT_RECORD = () => ({
  intake: '상담', actorTypeText: '당사자', actorType: '학생', actorNameChoice: OTHER, actorNameOther: '', actors: [],
  actorGroupId: 'group-1', actorMemberId: '',
  relTypeText: '상대방', relType: '학부모', relNameChoice: OTHER, relNameOther: '', related: [],
  relGroupId: 'group-1', relMemberId: '',
  placeText: '온라인', place: '온라인', placeOther: '',
  storeTypeText: '전화', storeType: '전화', storeOther: '',
  lvText: 'LV2', lv: 'LV2', ts: toLocalInputValue(nowISO()), summary: '',
  summaryOverview: '', summaryBackground: '', summaryIssues: '', summaryEvidenceList: '', summaryTeacherActions: '', summaryOther: '',
  signerLabel: '기기 봉인서명', sealReason: ''
});

const seedStrategyRecordDraftDefaultsFromRecords = (records: RecordItem[]) => {
  const first = records[0] ? ensureRecordV8(records[0]) as any : null;
  const actorSeed = first
    ? (Array.isArray(first.actors) && first.actors.length
      ? JSON.parse(JSON.stringify(first.actors))
      : (first.actor?.name ? [{ type: first.actor.type, name: first.actor.name }] : []))
    : [];
  const relatedSeed = first && Array.isArray(first.related) ? JSON.parse(JSON.stringify(first.related)) : [];
  return {
    actors: actorSeed,
    related: relatedSeed,
    ts: first?.ts ? toLocalInputValue(String(first.ts)) : toLocalInputValue(nowISO()),
    placeText: String(first?.place || '온라인'),
    place: (first?.place || '온라인') as PlaceType,
    placeOther: String(first?.placeOther || ''),
    storeTypeText: String(first?.storeType || '전화'),
    storeType: (first?.storeType || '전화') as StoreType,
    storeOther: String(first?.storeOther || ''),
    lvText: String(first?.lv || 'LV2'),
    lv: (first?.lv || 'LV2') as Sensitivity,
  };
};

const applyStrategyRecordDraftToComposer = (raw: string, opts?: { sourceInput?: string; contextRecords?: RecordItem[] }) => {
  const parsed = parseStrategyRecordDraftText(raw);
  const selectedRecords = getStrategySelectedRecords();
  const mergedContextRecords = mergeStrategyRecords(selectedRecords, Array.isArray(opts?.contextRecords) ? opts!.contextRecords! : []);
  const defaults = seedStrategyRecordDraftDefaultsFromRecords(mergedContextRecords);
  Object.assign(draftRecord, DEFAULT_RECORD(), defaults);

  draftRecord.summaryOverview = parsed.parts.overview;
  draftRecord.summaryBackground = parsed.parts.background;
  draftRecord.summaryIssues = parsed.parts.issues;
  draftRecord.summaryEvidenceList = parsed.parts.evidenceList;
  draftRecord.summaryTeacherActions = parsed.parts.teacherActions;
  draftRecord.summaryOther = parsed.parts.other;

  if (parsed.metadata.ts) draftRecord.ts = normalizeStrategyDraftTs(parsed.metadata.ts);
  const inferredPlace = parsed.metadata.placeChannel
    || inferStrategyPlaceFromContextRecords(mergedContextRecords)
    || extractStrategyPlaceHintFromInput(String(opts?.sourceInput || raw));
  if (inferredPlace) {
    draftRecord.placeText = inferredPlace;
    draftRecord.place = (PLACE_TYPES as any).includes(inferredPlace as any) ? inferredPlace as any : '기타' as any;
    draftRecord.placeOther = draftRecord.place === '기타' ? inferredPlace : '';
  }
  if (parsed.metadata.storeType) {
    draftRecord.storeTypeText = parsed.metadata.storeType;
    draftRecord.storeType = (STORE_TYPES as any).includes(parsed.metadata.storeType as any) ? parsed.metadata.storeType as any : '기타' as any;
    draftRecord.storeOther = draftRecord.storeType === '기타' ? parsed.metadata.storeType : '';
  }
  if (parsed.metadata.lv && (LVS as any).includes(parsed.metadata.lv as any)) {
    draftRecord.lvText = parsed.metadata.lv;
    draftRecord.lv = parsed.metadata.lv as any;
  }

  const sourceInput = String(opts?.sourceInput || raw);
  const actionNotes = extractStrategyActionNotes(sourceInput);
  const mentionedActors = resolveStrategyMentionedActors(sourceInput);
  const inferredPeople = extractStrategyPeopleFromInput(sourceInput);
  const actorNames = splitStrategyActorNames(parsed.metadata.actor);
  if (mentionedActors.length) {
    draftRecord.actors = addActorToList([], mentionedActors[0]);
  } else if (actorNames.length) {
    draftRecord.actors = actorNames.reduce((acc, name) => addActorToList(acc, { type: '학생' as any, name }), [] as ActorRef[]);
  } else if (inferredPeople.length) {
    draftRecord.actors = addActorToList([], { type: '학생' as any, name: inferredPeople[0] });
  }
  const relatedNames = splitStrategyActorNames(parsed.metadata.related);
  if (mentionedActors.length > 1) {
    draftRecord.related = mentionedActors.slice(1).reduce((acc, actor) => addActorToList(acc, actor), [] as ActorRef[]);
  } else if (relatedNames.length) {
    draftRecord.related = relatedNames.reduce((acc, name) => addActorToList(acc, { type: '학부모' as any, name }), [] as ActorRef[]);
  } else if (inferredPeople.length > 1) {
    draftRecord.related = inferredPeople.slice(1).reduce((acc, name) => addActorToList(acc, { type: '학생' as any, name }), [] as ActorRef[]);
  }

  if (actionNotes.length) {
    const currentTeacherActions = String(draftRecord.summaryTeacherActions || '').trim();
    const missingActionNotes = actionNotes.filter((note) => !currentTeacherActions.includes(note));
    if (!currentTeacherActions) {
      draftRecord.summaryTeacherActions = actionNotes
        .map((note, index) => `${index + 1}. ${note}`)
        .join('\n');
    } else if (missingActionNotes.length) {
      draftRecord.summaryTeacherActions = [
        currentTeacherActions,
        ...missingActionNotes.map((note, index) => `${index + 1}. ${note}`),
      ].join('\n');
    }
  }

  const summaryPack = buildSummaryFromDraftParts(draftRecord as any);
  draftRecord.summary = summaryPack.text || parsed.raw;
  return { parsed, summaryText: summaryPack.text };
};
const DEFAULT_CASE = () => ({
  title: '', query: '', timeFrom: '', timeTo: '', maxResults: 80, actors: [],
  onlyMainActor: false,
  sensFilterText: 'any', sensFilter: 'any', statusText: '진행중', status: '진행중',
  mainActorKey: '', relatedActorKey: ''
});

function prepareRecordDraftForSeal() {
  const placeText = String((draftRecord as any).placeText || '').trim();
  const storeText = String((draftRecord as any).storeTypeText || '').trim();
  const lvText = String((draftRecord as any).lvText || '').trim();
  const tsTxt = String(draftRecord.ts || '').trim();
  const summaryPack = buildSummaryFromDraftParts(draftRecord as any);
  const actorsClean = getDraftActorsForSave(draftRecord as any);

  if (tsTxt.length < 10) return { error: '시간을 입력하세요' };
  if (!actorsClean.length) return { error: '주체를 1명 이상 추가하세요' };
  if (summaryPack.text.length < 4) return { error: '내용을 4글자 이상 입력하세요' };
  if (!placeText || !storeText || !lvText) return { error: '필수 정보를 입력하세요' };

  const placeIsKnown = (PLACE_TYPES as any).includes(placeText as any);
  const storeIsKnown = (STORE_TYPES as any).includes(storeText as any);
  const place: PlaceType = placeIsKnown ? (placeText as any) : ('기타' as any);
  const placeOther = placeText === '기타' ? String(draftRecord.placeOther || '').trim() : (placeIsKnown ? '' : placeText);
  const storeType: StoreType = storeIsKnown ? (storeText as any) : ('기타' as any);
  const storeOther = storeText === '기타' ? String(draftRecord.storeOther || '').trim() : (storeIsKnown ? '' : storeText);
  if (place === '기타' && !placeOther) return { error: '장소 상세(기타)를 입력하세요' };
  if (storeType === '기타' && !storeOther) return { error: '보관형태 상세(기타)를 입력하세요' };

  const relatedClean = (draftRecord.related || []).filter((a) => String((a as any)?.name || '').trim().length > 0);

  return {
    actorsClean,
    place,
    placeOther,
    storeType,
    storeOther,
    lvText,
    relatedClean,
    summary: summaryPack.text,
    summaryParts: summaryPack.parts,
    tsISO: fromLocalInputValue(draftRecord.ts),
  };
}

function prepareRecordEditForSeal() {
  const placeText = String((draftRecordEdit as any).placeText || '').trim();
  const storeText = String((draftRecordEdit as any).storeTypeText || '').trim();
  const lvText = String((draftRecordEdit as any).lvText || '').trim();
  const tsTxt = String(draftRecordEdit.ts || '').trim();
  const summaryPack = buildSummaryFromDraftParts(draftRecordEdit as any);
  const actorsClean = getDraftActorsForSave(draftRecordEdit as any);

  if (tsTxt.length < 10) return { error: '기록 시각을 입력하세요' };
  if (!actorsClean.length) return { error: '주체를 1명 이상 추가하세요' };
  if (summaryPack.text.length < 4) return { error: '내용을 4글자 이상 입력하세요' };
  if (!placeText || !storeText || !lvText) return { error: '필수 정보를 입력하세요' };

  const placeIsKnown = (PLACE_TYPES as any).includes(placeText as any);
  const storeIsKnown = (STORE_TYPES as any).includes(storeText as any);
  const place: PlaceType = placeIsKnown ? (placeText as any) : ('기타' as any);
  const placeOther = placeText === '기타' ? String(draftRecordEdit.placeOther || '').trim() : (placeIsKnown ? '' : placeText);
  const storeType: StoreType = storeIsKnown ? (storeText as any) : ('기타' as any);
  const storeOther = storeText === '기타' ? String(draftRecordEdit.storeOther || '').trim() : (storeIsKnown ? '' : storeText);
  if (place === '기타' && !placeOther) return { error: '장소 상세(기타)를 입력하세요' };
  if (storeType === '기타' && !storeOther) return { error: '보관형태 상세(기타)를 입력하세요' };

  const relatedClean = (draftRecordEdit.related || []).filter((a) => String((a as any)?.name || '').trim().length > 0);

  return {
    actorsClean,
    place,
    placeOther,
    storeType,
    storeOther,
    lvText,
    relatedClean,
    summary: summaryPack.text,
    summaryParts: summaryPack.parts,
    tsISO: fromLocalInputValue(draftRecordEdit.ts),
  };
}

/* ---------- event binding ---------- */
let _bound = false;

// backup/restore (file)
let _restoreFileText: string | null = null;
let _restoreFileName: string | null = null;
const resetRestoreFileSelection = () => {
  _restoreFileText = null;
  _restoreFileName = null;
};
function bindEvents() {
  if (_bound) return; _bound = true;

  const finalizeCreateRecord = async () => {
    const prep = prepareRecordDraftForSeal() as any;
    if (prep.error) return toast(prep.error);

    const { record, error } = buildRecordFromDraft({
      tsISO: prep.tsISO,
      storeType: prep.storeType,
      storeOther: prep.storeOther,
      lv: prep.lvText as any,
      actors: prep.actorsClean,
      related: prep.relatedClean,
      place: prep.place,
      placeOther: prep.placeOther,
      summary: prep.summary,
      summaryParts: prep.summaryParts,
    }, () => uid('REC'));
    if (error) return toast(error);

    const sealed = await sealNewRecord(record!, {
      sealedAt: nowISO(),
      signerLabel: String((draftRecord as any).signerLabel || '').trim() || '기기 봉인서명',
      reason: String((draftRecord as any).sealReason || '').trim() || '초기 기록 봉인',
    });

    S.records.unshift(sealed as any);
    const sel = getSelectedCase();
    if (sel) S.cases[sel.id] = await addRecordsToCase(sel, S.records, [sealed.id]);
    await saveState(S);

    draftRecord.summary = '';
    draftRecord.summaryOverview = '';
    draftRecord.summaryBackground = '';
    draftRecord.summaryIssues = '';
    draftRecord.summaryEvidenceList = '';
    draftRecord.summaryTeacherActions = '';
    draftRecord.summaryOther = '';
    draftRecord.actors = [];
    draftRecord.actorNameChoice = OTHER;
    draftRecord.actorNameOther = '';
    draftRecord.relNameChoice = OTHER;
    draftRecord.relNameOther = '';
    draftRecord.related = [];
    draftRecord.ts = toLocalInputValue(nowISO());
    (draftRecord as any).signerLabel = '기기 봉인서명';
    (draftRecord as any).sealReason = '';
    (ui as any).lastSavedRecordId = sealed.id;
    ui.recordComposerOpen = false;

    closeSignatureModal();
    render();

    openSignSuccessModal(
      '성공적으로 서명 및 인증이 완료되었습니다!',
      sel ? '기록이 저장되고 선택한 컬렉션에도 자동 반영되었어요.' : '기록이 저장되었습니다.'
    );

    const sealVerify = verifyRecordIntegrity(sealed as any) as any;
    toast(`봉인 완료 ✅ ${sealVerify.trusted ? '기기서명 확인' : '봉인 저장됨'}`);
    log('record sealed', sealed.id, sealVerify.verificationStatus);
  };

  const finalizeAmendRecord = async () => {
    const id = String(ui.recordEditId || '').trim();
    if (!id) return toast('수정할 기록을 찾을 수 없어요');
    const idx = S.records.findIndex((x) => x.id === id);
    if (idx < 0) return toast('수정할 기록을 찾을 수 없어요');

    const prep = prepareRecordEditForSeal() as any;
    if (prep.error) return toast(prep.error);

    const current = ensureRecordV8(S.records[idx]) as any;
    const nextRecord = {
      ...current,
      id: current.id,
      ts: prep.tsISO,
      actor: prep.actorsClean[0],
      actors: prep.actorsClean,
      related: prep.relatedClean,
      place: prep.place,
      placeOther: prep.placeOther,
      storeType: prep.storeType,
      storeOther: prep.storeOther,
      lv: prep.lvText as any,
      summary: prep.summary,
      summaryParts: prep.summaryParts,
    } as any;

    const amended = await amendSignedRecord(current, nextRecord, {
      sealedAt: nowISO(),
      signerLabel: String((draftRecordEdit as any).signerLabel || '').trim() || '기기 봉인서명',
      reason: String((draftRecordEdit as any).sealReason || '').trim() || '기록 정정 및 재봉인',
    });

    S.records[idx] = amended as any;
    ui.viewRecordId = amended.id;
    ui.recordEditId = null;
    ui.recordModalTab = 'history';
    ui.recEditRelatedOpen = false;
    resetRecordEditDraft();
    await saveState(S);

    closeSignatureModal();
    render();
    openRecordModal();

    openSignSuccessModal('성공적으로 서명 및 인증이 완료되었습니다!', '수정 내용이 저장되고 새 revision으로 재봉인되었어요.');

    const amendVerify = verifyRecordIntegrity(amended as any) as any;
    toast(`정정 봉인 완료 ✅ ${amendVerify.trusted ? '기기서명 확인' : '재봉인 저장됨'}`);
    log('record amended', amended.id, amendVerify.verificationStatus);
  };

  const click: Record<string, (btn: HTMLElement) => void | Promise<void>> = {
    'toast-action': () => runToastAction(),
    'confirm-yes': () => closeConfirm(true), 'confirm-no': () => closeConfirm(false),

    'close-record': () => (closeRecordModal(), render()),
    'open-record-composer': () => { ui.recordComposerOpen = true; render(); window.setTimeout(() => { (document.getElementById('recordSummaryOverview') as HTMLTextAreaElement | null)?.focus(); }, 0); log('record composer modal open'); },
    'close-record-composer': () => { ui.recordComposerOpen = false; closeDlg('recordComposerModal'); render(); log('record composer modal close'); },
    'clear-record-filters': () => (ui.recFilterActor = ui.recFilterPlace = ui.recFilterKeyword = '', ui.recFilterActorDraft = ui.recFilterPlaceDraft = ui.recFilterKeywordDraft = '', render(), log('record filters cleared')),
    'apply-record-filters': () => (ui.recFilterActor = ui.recFilterActorDraft, ui.recFilterPlace = ui.recFilterPlaceDraft, ui.recFilterKeyword = ui.recFilterKeywordDraft, render(), log('record filters applied')),
    'apply-update-filters': () => (ui.updFilterActor = ui.updFilterActorDraft, ui.updFilterPlace = ui.updFilterPlaceDraft, ui.updFilterKeyword = ui.updFilterKeywordDraft, render(), log('update filters applied')),
    'clear-update-filters': () => (ui.updFilterActor = ui.updFilterPlace = ui.updFilterKeyword = '', ui.updFilterActorDraft = ui.updFilterPlaceDraft = ui.updFilterKeywordDraft = '', render(), log('update filters cleared')),
    'close-timeline-detail': () => (closeTimelineModal(), render()),

    tab: async (btn) => {
      const rawTab = String(btn.dataset.tab || '').trim();
      const nextTab = (rawTab === 'cases' || rawTab === 'records' ? 'cases' : rawTab === 'legal' ? 'legal' : 'home') as any;
      S.tab = nextTab;
      if (nextTab === 'records') ui.evidenceTab = ui.evidenceTab || 'write';
      if (nextTab === 'cases') {
        if (rawTab === 'records') ui.caseTab = 'records' as any;
        else ui.caseTab = (ui.caseTab === 'list' || ui.caseTab === 'proof' || ui.caseTab === 'records') ? ui.caseTab : 'records';
      }
      await saveState(S); log(`tab -> ${S.tab}`); render();
    },

    'switch-evidence-tab': (btn) => {
      const next = btn.dataset.evidenceTab === 'list' ? 'list' : 'write';
      ui.evidenceTab = next;
      S.tab = 'records' as any;
      render();
      log('evidence tab ->', next);
    },
    'switch-record-modal-tab': (btn) => {
      const next = btn.dataset.recordModalTab === 'history' ? 'history' : btn.dataset.recordModalTab === 'edit' ? 'edit' : 'current';
      ui.recordModalTab = next as any;
      render();
      openRecordModal();
      log('record modal tab ->', next);
    },
    'switch-case-tab': (btn) => {
      const raw = String(btn.dataset.caseTab || '').trim();
      if (raw === 'create') {
        S.tab = 'cases' as any;
        ui.caseTab = 'list' as any;
        ui.caseCreateOpen = true;
        render();
        openCaseCreateModal();
        log('case create modal open (legacy tab route)');
        return;
      }
      const next = raw === 'list' ? 'list' : raw === 'proof' ? 'proof' : 'records';
      ui.caseTab = next as any;
      S.tab = 'cases' as any;
      render();
      log('case tab ->', next);
    },
    'switch-legal-tab': (btn) => {
      const next = 'simulation';
      (ui as any).legalTab = next;
      ensureSimulationDraft();
      const currentSimulationCaseId = String((ui as any).simulationCaseId || '').trim();
      if (currentSimulationCaseId && S.cases[currentSimulationCaseId]) {
        (ui as any).simulationSelectedRecordIds = getSimulationCaseRecordIds(currentSimulationCaseId);
      } else if (S.selectedCaseId && S.cases[S.selectedCaseId] && !(Array.isArray((ui as any).simulationSelectedRecordIds) && ((ui as any).simulationSelectedRecordIds as string[]).length)) {
        (ui as any).simulationCaseId = S.selectedCaseId;
        (ui as any).simulationSelectedRecordIds = getSimulationCaseRecordIds(S.selectedCaseId);
      } else {
        reseedSimulationSelection(false);
      }
      S.tab = 'home' as any;
      render();
      log('legal tab ->', next);
    },
    'set-legal-agent-mode': (btn) => {
      const next = String(btn.dataset.mode || '').trim() === 'record' ? 'record' : 'analysis';
      const current = getStrategyChatMode();
      if (current !== next && next === 'analysis') {
        clearStrategyChat();
      }
      (ui as any).strategyChatMode = next;
      closeStrategyMentionMenu();
      closeStrategyRecordDraftEditor();
      render();
      focusStrategyChatComposer();
      toast(next === 'record' ? '기록모드로 전환했어요' : '분석모드로 전환했어요');
    },
    'open-simulation-picker': () => {
      closeStrategyChatModelMenu();
      (ui as any).simulationPickerOpen = true;
      (ui as any).simulationPickerQuery = '';
      syncSimulationPickerSelectionFromLive();
      render();
      log('simulation picker open');
    },
    'close-simulation-picker': () => {
      (ui as any).simulationPickerOpen = false;
      (ui as any).simulationPickerQuery = '';
      (ui as any).simulationPickerSelectedRecordIds = [];
      closeDlg('simulationPickerModal');
      render();
      log('simulation picker close');
    },
    'simulation-apply-picker': () => {
      const appliedIds = getSimulationPickerSelectedIds();
      (ui as any).simulationSelectedRecordIds = appliedIds;
      (ui as any).simulationResult = null;
      markSimulationDirty();
      (ui as any).simulationPickerOpen = false;
      (ui as any).simulationPickerQuery = '';
      (ui as any).simulationPickerSelectedRecordIds = [];
      closeDlg('simulationPickerModal');
      render();
      toast(`기록 선택 ${appliedIds.length}개를 적용했어요`);
      log('simulation picker apply', String(appliedIds.length));
    },

'simulation-set-preset': (btn) => {
  const preset = String(btn.dataset.preset || '').trim() === 'shield'
    ? 'shield'
    : String(btn.dataset.preset || '').trim() === 'assertive'
      ? 'assertive'
      : 'balanced';
  applySimulationPreset(preset as SimulationPreset);
  (ui as any).simulationResult = null;
  markSimulationDirty();
  render();
  log('simulation preset ->', preset);
},
'simulation-toggle-record': (btn) => {
  const id = String(btn.dataset.id || '').trim();
  if (!id) return;
  if ((ui as any).simulationPickerOpen) {
    const selected = getSimulationPickerSelectedIds();
    const has = selected.includes(id);
    setSimulationPickerSelectedIds(has ? selected.filter((item) => item !== id) : [...selected, id]);
    render();
    log('simulation picker toggle record', id, has ? 'off' : 'on');
    return;
  }
  const selected = Array.isArray((ui as any).simulationSelectedRecordIds) ? ((ui as any).simulationSelectedRecordIds as string[]) : [];
  const has = selected.includes(id);
  (ui as any).simulationSelectedRecordIds = has ? selected.filter((item) => item !== id) : [...selected, id];
  (ui as any).simulationResult = null;
  markSimulationDirty();
  render();
  log('simulation toggle record', id, has ? 'off' : 'on');
},
'simulation-reset-to-case': () => {
  const caseId = String((ui as any).simulationCaseId || '').trim();
  if (!caseId || !S.cases[caseId]) return toast('기준 컬렉션을 먼저 선택하세요');
  if ((ui as any).simulationPickerOpen) {
    const nextIds = getSimulationCaseRecordIds(caseId);
    setSimulationPickerSelectedIds(nextIds);
    render();
    toast('선택 모달 안에서 컬렉션 기록으로 다시 채웠어요');
    log('simulation picker reset to case', caseId);
    return;
  }
  applySimulationCaseSelection(caseId);
  render();
  toast('선택한 컬렉션의 기록으로 다시 채웠어요');
  log('simulation reset to case', caseId);
},
'simulation-auto-pick': () => {
  const baseRecords = getSimulationBaseRecords();
  const nextIds = baseRecords.slice(0, 5).map((record: any) => String(record.id || ''));
  if ((ui as any).simulationPickerOpen) {
    setSimulationPickerSelectedIds(nextIds);
    render();
    toast('선택 모달 안에서 최근/핵심 기록을 다시 골랐어요');
    log('simulation picker auto pick');
    return;
  }
  (ui as any).simulationCaseId = null;
  (ui as any).simulationSelectedRecordIds = nextIds;
  (ui as any).simulationResult = null;
  markSimulationDirty();
  render();
  toast('전체 기록에서 최근/핵심 기록을 다시 묶었어요');
  log('simulation auto pick');
},
'simulation-clear-selection': () => {
  if ((ui as any).simulationPickerOpen) {
    (ui as any).simulationPickerSelectedRecordIds = [];
    render();
    toast('선택 모달 안의 기록을 비웠어요');
    log('simulation picker clear selection');
    return;
  }
  (ui as any).simulationSelectedRecordIds = [];
  (ui as any).simulationResult = null;
  markSimulationDirty();
  render();
  toast('선택한 기록을 비웠어요');
  log('simulation clear selection');
},
'simulation-reset-scenario': () => {
  const draft = ensureSimulationDraft();
  const keepFilter = String(draft.evidenceFilter || '');
  Object.assign(draft, SIMULATION_DEFAULTS, { evidenceFilter: keepFilter });
  (ui as any).simulationResult = null;
  markSimulationDirty();
  render();
  toast('대응 시나리오 값을 기본값으로 되돌렸어요');
  log('simulation reset');
},
'simulation-calc': () => {
  reseedSimulationSelection(false);
  const result = computeSimulationResult();
  if (!result) return toast('먼저 기록을 1개 이상 묶어주세요');
  (ui as any).simulationResult = result;
  (ui as any).simulationDirty = false;
  render();
  toast('대응 시나리오 계산 완료');
  log('simulation calculated', String(result.responseIndex));
},
    'send-strategy-chat': async () => {
      if ((ui as any).strategyChatPending) return;
      await sendStrategyAgentMessage();
    },
    'open-strategy-input-guide': () => {
      (ui as any).strategyInputGuideOpen = true;
      render();
    },
    'close-strategy-input-guide': () => {
      (ui as any).strategyInputGuideOpen = false;
      closeDlg('strategyInputGuideModal');
      render();
    },
    'pick-strategy-mention': (btn) => {
      const groupId = String(btn.dataset.groupId || '').trim();
      const memberId = String(btn.dataset.memberId || '').trim();
      const name = String(btn.dataset.name || '').trim();
      const groupLabel = String(btn.dataset.groupLabel || '').trim();
      if (!groupId || !memberId || !name) return;
      applyStrategyMentionSuggestion({ groupId, memberId, name, groupLabel });
    },
    'draft-strategy-action': (btn) => {
      (ui as any).strategyActionDraft = String((btn as HTMLInputElement | null)?.value || '');
    },
    'apply-strategy-action': () => {
      applyStrategyActionPrompt();
    },
    'close-strategy-action': () => {
      closeStrategyActionPrompt();
      render();
      focusStrategyChatComposer();
    },
    'edit-strategy-record-draft': (btn) => {
      const messageId = String(btn.dataset.id || '').trim();
      if (!messageId) return;
      openStrategyRecordDraftEditor(messageId);
    },
    'cancel-edit-strategy-record-draft': () => {
      closeStrategyRecordDraftEditor();
      render();
    },
    'save-edit-strategy-record-draft': (btn) => {
      const messageId = String(btn.dataset.id || '').trim();
      if (!messageId) return;
      saveStrategyRecordDraftEdit(messageId);
    },
    'save-strategy-record-draft': (btn) => {
      const messageId = String(btn.dataset.id || '').trim();
      if (!messageId) return;
      saveStrategyRecordDraftToArchive(messageId);
    },
    'download-strategy-models': async () => {
      if ((ui as any).strategyModelDownloadPending) return;
      await downloadStrategyModels();
    },
    'focus-strategy-chat': () => {
      (ui as any).strategyModelDownloadMessage = '';
      render();
      focusStrategyChatComposer();
    },
    'toggle-strategy-model-menu': () => {
      (ui as any).strategyChatModelMenuOpen = !(ui as any).strategyChatModelMenuOpen;
      render();
      log('strategy model menu', (ui as any).strategyChatModelMenuOpen ? 'open' : 'close');
    },
    'toggle-strategy-synthetic-cache': () => {
      (ui as any).strategySyntheticCacheEnabled = true;
      persistStrategySyntheticCacheSetting(true);
      maybeStartStrategyBackendPrewarm('toggle_on');
      render();
      toast('이제 Resident Cache Mode만 사용해요');
    },
    'toggle-strategy-backend-mode': () => {
      const draft = {
        ...defaultStrategyLlamaServerConfig(),
        ...(((ui as any).strategyLlamaServerConfig || defaultStrategyLlamaServerConfig()) as StrategyLlamaServerConfigDraft),
      };
      (ui as any).strategyBackendMode = 'llama-server';
      (ui as any).strategyLlamaServerConfig = draft;
      persistStrategyBackendSettings({
        mode: 'llama-server',
        llamaServer: draft,
      });
      render();
      toast('Resident llama-server backend로 고정했어요');
    },
    'toggle-strategy-perf-panel': () => {
      (ui as any).strategyPerfCollapsed = !((ui as any).strategyPerfCollapsed === true);
      render();
    },
    'reset-strategy-perf-lab': () => {
      resetStrategyPerfLabState();
      render();
      toast('실험용 지표를 초기화했어요');
    },
    'save-strategy-backend-settings': () => {
      const mode = getStrategyBackendMode();
      const llamaServer = getStrategyLlamaServerConfig();
      if (!llamaServer.hyperclovaUrl || !llamaServer.roosyUrl) {
        toast('llama-server endpoint를 모두 입력해주세요');
        return;
      }
      (ui as any).strategyBackendMode = mode;
      (ui as any).strategyLlamaServerConfig = {
        ...defaultStrategyLlamaServerConfig(),
        ...llamaServer,
        hyperclovaSlot: llamaServer.hyperclovaSlot == null ? '' : String(llamaServer.hyperclovaSlot),
        roosySlot: llamaServer.roosySlot == null ? '' : String(llamaServer.roosySlot),
        startupTimeoutMs: String(llamaServer.startupTimeoutMs),
        requestTimeoutMs: String(llamaServer.requestTimeoutMs),
      };
      persistStrategyBackendSettings({
        mode,
        llamaServer: (ui as any).strategyLlamaServerConfig,
      });
      maybeStartStrategyBackendPrewarm('settings_save');
      render();
      toast('llama-server backend 설정을 저장했어요');
    },
    'select-strategy-model': (btn) => {
      const next = normalizeStrategyModel(btn.dataset.model || '');
      (ui as any).strategyChatModel = next;
      closeStrategyChatModelMenu();
      render();
      toast('ROOSY-Hybrid로 맞춰뒀어요');
      log('strategy model ->', next);
    },
    'clear-strategy-chat': () => {
      clearStrategyChat();
      render();
      toast('AI 민원 법무팀 에이전트 대화를 비웠어요');
      log('strategy chat cleared');
    },
    'prime-strategy-chat': async () => {
      if ((ui as any).strategyChatPending) return;
      await sendStrategyAgentMessage(STRATEGY_DEFAULT_PROMPT);
    },
    'save-strategy-thread-package': async () => {
      const activeId = getActiveStrategyThreadPackageId();
      const saved = await persistStrategyThreadPackage(activeId || undefined, { activate: true });
      if (!saved) return;
      render();
    },
    'detach-strategy-thread-package': () => {
      startFreshStrategyThread();
    },
    'open-strategy-thread-package': async (btn) => {
      const id = String(btn.dataset.id || '').trim();
      if (!id) return;
      await openStrategyThreadPackage(id);
    },
    'delete-strategy-thread-package': async (btn) => {
      const id = String(btn.dataset.id || '').trim();
      if (!id) return;
      await deleteStrategyThreadPackage(id);
    },
    'window-minimize': async () => {
      const win = currentDesktopWindow();
      if (!win) return;
      try { await win.minimize(); } catch (e) { log('window minimize failed', e); }
    },
    'window-toggle-maximize': async () => {
      const win = currentDesktopWindow();
      if (!win) return;
      try { await win.toggleMaximize(); } catch (e) { log('window toggle maximize failed', e); }
    },
    'window-close': async () => {
      const win = currentDesktopWindow();
      if (!win) return;
      try { await win.close(); } catch (e) { log('window close failed', e); }
    },
    'open-settings': () => (ui.settingsOpen = true, render(), log('settings modal open')),
    'close-settings': () => (ui.settingsOpen = false, resetScreenPinSettingsDraft(), closeDlg('settingsModal'), log('settings modal close')),
    'open-updates-note': () => (ui.updatesNoteOpen = true, render(), log('updates note modal open')),
    'close-updates-note': () => (ui.updatesNoteOpen = false, closeDlg('updateNotesModal'), render(), log('updates note modal close')),
    'open-screen-lock': () => {
      resetScreenPinModalDraft();
      if (!hasScreenPin()) {
        ui.pinLocked = false;
        ui.pinModalOpen = true;
        render();
        focusScreenPinInput();
        toast('먼저 PIN을 설정해주세요');
        log('screen pin modal open (setup)');
        return;
      }
      ui.pinLocked = true;
      ui.pinModalOpen = true;
      render();
      focusScreenPinInput();
      toast('앱 잠금이 켜졌어요');
      log('screen locked');
    },
    'close-screen-pin': () => {
      if (ui.pinLocked) return;
      ui.pinModalOpen = false;
      resetScreenPinModalDraft();
      closeDlg(SCREEN_PIN_MODAL_ID);
      render();
      log('screen pin modal close');
    },
    'submit-screen-pin': () => {
      const savedPin = readScreenPin();
      const entered = normalizeScreenPin(String(ui.pinEntryDraft || ''));
      const confirmPin = normalizeScreenPin(String(ui.pinConfirmDraft || ''));
      if (!savedPin) {
        if (!isValidScreenPin(entered)) return toast('PIN은 숫자 4자리여야 해요');
        if (entered !== confirmPin) return toast('PIN이 서로 달라요');
        if (!saveScreenPin(entered)) return toast('PIN 저장에 실패했어요');
        ui.pinLocked = false;
        ui.pinModalOpen = false;
        resetScreenPinModalDraft();
        render();
        toast('PIN 설정 완료');
        log('screen pin set');
        return;
      }
      if (entered !== savedPin) {
        ui.pinEntryDraft = '';
        render();
        focusScreenPinInput();
        toast('PIN이 일치하지 않아요');
        log('screen unlock failed');
        return;
      }
      ui.pinLocked = false;
      ui.pinModalOpen = false;
      resetScreenPinModalDraft();
      closeDlg(SCREEN_PIN_MODAL_ID);
      render();
      toast('앱 잠금이 해제되었어요');
      log('screen unlocked');
    },
    'save-screen-pin': async () => {
      const nextPin = normalizeScreenPin(String(ui.pinSettingsDraft || ''));
      const confirmPin = normalizeScreenPin(String(ui.pinSettingsConfirmDraft || ''));
      const hadPin = hasScreenPin();
      if (!isValidScreenPin(nextPin)) return toast('PIN은 숫자 4자리여야 해요');
      if (nextPin !== confirmPin) return toast('PIN이 서로 달라요');
      if (!saveScreenPin(nextPin)) return toast('PIN 저장에 실패했어요');
      resetScreenPinSettingsDraft();
      render();
      toast(hadPin ? 'PIN 변경 완료' : 'PIN 설정 완료');
      log(hadPin ? 'screen pin changed' : 'screen pin created');
    },
    'clear-screen-pin': async () => {
      if (!hasScreenPin()) return toast('설정된 PIN이 없어요');
      if (!(await openConfirm('설정된 PIN을 삭제할까요?'))) return;
      clearScreenPin();
      ui.pinLocked = false;
      ui.pinModalOpen = false;
      resetScreenPinModalDraft();
      resetScreenPinSettingsDraft();
      closeDlg(SCREEN_PIN_MODAL_ID);
      render();
      toast('PIN 삭제 완료');
      log('screen pin cleared');
    },
    'open-class-roster': () => {
      ui.classRosterDraft = cloneRelationshipGroups(getRelationshipGroups());
      ui.classRosterGroupId = String(ui.classRosterDraft[0]?.id || 'group-1');
      ui.classRosterOpen = true;
      render();
      log('class roster modal open');
    },
    'close-class-roster': () => {
      ui.classRosterOpen = false;
      ui.classRosterDraft = cloneRelationshipGroups(getRelationshipGroups());
      ui.classRosterGroupId = String(ui.classRosterDraft[0]?.id || 'group-1');
      closeDlg('classRosterModal');
      render();
      log('class roster modal close');
    },
    'save-class-roster': async () => {
      const draft = ensureClassRosterDraft().map((group: any, groupIndex: number) => ({
        id: String(group?.id || `group-${groupIndex + 1}`),
        title: String(group?.title || `그룹${groupIndex + 1}`).trim() || `그룹${groupIndex + 1}`,
        members: (Array.isArray(group?.members) ? group.members : [])
          .map((member: any, memberIndex: number) => ({
            id: String(member?.id || `${String(group?.id || `group-${groupIndex + 1}`)}-member-${memberIndex + 1}`),
            name: String(member?.name || '').trim(),
          }))
          .filter((member: any) => member.name),
      }));
      S.relationshipGroups = cloneRelationshipGroups(draft as any);
      const flattenedRoster = draft.flatMap((group: any) => group.members.map((member: any) => member.name)).slice(0, 40);
      S.classRoster = Array.from({ length: 40 }, (_, i) => String(flattenedRoster[i] || ''));
      ui.classRosterDraft = cloneRelationshipGroups(S.relationshipGroups);
      ui.classRosterGroupId = String(ui.classRosterDraft.find((group: any) => String(group?.id || '') === String(ui.classRosterGroupId || ''))?.id || ui.classRosterDraft[0]?.id || 'group-1');
      ui.classRosterOpen = false;
      syncClassRosterFieldDefaults();
      await SR();
      const memberCount = draft.reduce((acc: number, group: any) => acc + group.members.length, 0);
      toast(`관계 관리 저장 완료 ✅ ${memberCount}명 등록`);
      log('relationship groups saved', memberCount);
    },
    'add-relationship-member': (btn) => {
      const draft = ensureClassRosterDraft() as any[];
      const groupIndex = Number(btn.dataset.groupIndex ?? '-1');
      if (Number.isNaN(groupIndex) || groupIndex < 0 || groupIndex >= draft.length) return;
      const group = draft[groupIndex];
      ui.classRosterGroupId = String(group?.id || ui.classRosterGroupId || '');
      group.members = Array.isArray(group.members) ? group.members : [];
      group.members.push({ id: uid(`rel_${group.id}`), name: '' });
      render();
      updateClassRosterCountUI();
    },
    'add-relationship-group': () => {
      const draft = ensureClassRosterDraft() as any[];
      if (draft.length >= 12) {
        toast('그룹은 최대 12개까지 만들 수 있어요.');
        return;
      }
      const nextIndex = draft.length + 1;
      const groupId = uid('group');
      draft.push({
        id: groupId,
        title: `그룹${nextIndex}`,
        members: [],
      });
      ui.classRosterGroupId = groupId;
      render();
      updateClassRosterCountUI();
    },
    'select-relationship-group': (btn) => {
      const draft = ensureClassRosterDraft() as any[];
      const groupId = String(btn.dataset.groupId || '').trim();
      if (!groupId) return;
      if (!draft.some((group: any) => String(group?.id || '') === groupId)) return;
      ui.classRosterGroupId = groupId;
      render();
    },
    'remove-relationship-member': (btn) => {
      const draft = ensureClassRosterDraft() as any[];
      const groupIndex = Number(btn.dataset.groupIndex ?? '-1');
      const memberIndex = Number(btn.dataset.memberIndex ?? '-1');
      if (Number.isNaN(groupIndex) || groupIndex < 0 || groupIndex >= draft.length) return;
      if (Number.isNaN(memberIndex) || memberIndex < 0) return;
      const group = draft[groupIndex];
      group.members = (Array.isArray(group.members) ? group.members : []).filter((_: any, index: number) => index !== memberIndex);
      render();
      updateClassRosterCountUI();
    },

    'open-case-create': () => (S.tab = 'cases' as any, ui.caseTab = 'list', ui.caseCreateOpen = true, render(), openCaseCreateModal(), void saveState(S), log('case create modal open')),
    'close-case-create': () => (closeCaseCreateModal(), render(), log('case create modal close')),

    'saved-close': () => closeDlg('savedModal'),
    'saved-view-record': () => { const id = (ui as any).lastSavedRecordId as string | undefined; closeDlg('savedModal'); if (!id) return; ui.viewRecordId = id; ui.recordModalTab = 'current'; render(); openRecordModal(); log('saved modal -> view record', id); },
    'close-signature-modal': () => closeSignatureModal(),
    'close-sign-success': () => closeDlg(SIGN_SUCCESS_MODAL_ID),
    'confirm-signature-submit': async () => {
      if (ui.signatureModalMode === 'amend') return await finalizeAmendRecord();
      return await finalizeCreateRecord();
    },
    'case-created-close': () => closeDlg('caseCreatedModal'),
    'case-created-open': () => { closeDlg('caseCreatedModal'); S.tab = 'cases' as any; ui.caseTab = 'list'; void saveState(S); render(); },
    'case-created-open-paper': async () => { closeDlg('caseCreatedModal'); const c = mustCase(); if (!c) return; ui.paperCaseId = c.id; ui.caseTab = 'proof' as any; S.tab = 'cases' as any; render(); log('paper proof tab open (case created modal)', c.id); },

    backup: async () => {
      const envelope = await buildSignedBackupEnvelope(S as any);
      const json = JSON.stringify(envelope, null, 2);
      const ts = nowISO().replace(/[:.]/g, '-').replace('T', '_').replace('Z', '');
      const suggested = `roosycozy_backup_${ts}.json`;

      const path = await saveDialog({
        defaultPath: suggested,
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (!path) return;

      try {
        const saved = await invoke<string>('export_backup_json', { args: { fileName: path, json } });
        toast('백업 파일 저장됨 ✅');
        log('backup saved', saved);
      } catch (e) {
        toast('백업 저장에 실패했어요');
        log('backup save failed', e);
      }
    },
    'load-sample': async () => {
      if (!(await openConfirm('샘플 데이터를 불러올까요?(현재 데이터는 샘플로 덮어써집니다)'))) return;
      try {
        const pack = await loadSamplePackJSON();
        const next = normalizeState(pack as any); next.tab = 'cases'; next.selectedCaseId = null;
        setState(next); await saveState(S); syncDraftDefaults(); render(); toast('샘플 데이터를 불러왔어요 ✅'); log('sample loaded');
      } catch (e) { log('sample load failed', e); toast('샘플 불러오기에 실패했어요'); }
    },

    'open-restore': () => {
      resetRestoreFileSelection();
      ui.settingsOpen = false; closeDlg('settingsModal');
      openDlg('restoreModal');
      const info = document.getElementById('restoreFileName');
      if (info) info.textContent = _restoreFileName ? `선택됨: ${_restoreFileName}` : '선택된 파일 없음';
      log('restore modal open');
    },

    'pick-restore-file': async () => {
      try {
        const picked = await openDialog({
          directory: false,
          multiple: false,
          filters: [{ name: 'JSON', extensions: ['json'] }],
        });
        if (!picked || Array.isArray(picked)) return;

        const restoreText = await invoke<string>('import_backup_json', { args: { filePath: picked } });
        _restoreFileText = String(restoreText || '');
        _restoreFileName = String(picked).split(/[\\/]/).pop() || 'backup.json';
        const info = document.getElementById('restoreFileName');
        if (info) info.textContent = `선택됨: ${_restoreFileName}`;
        toast('백업 파일 선택됨');
        log('restore file loaded (dialog)', _restoreFileName || '');
      } catch (e) {
        toast('백업 파일을 불러오지 못했어요');
        log('restore file pick failed', e);
      }
    },

    'close-restore': () => {
      resetRestoreFileSelection();
      const input = document.getElementById('restoreFile') as HTMLInputElement | null;
      if (input) input.value = '';
      closeDlg('restoreModal');
    },

    'do-restore': async () => {
      const parsed = safeParseJSON(_restoreFileText || '');
      if (!parsed || typeof parsed !== 'object') return toast('백업 파일을 먼저 선택하세요');
      if (!(await openConfirm('선택한 백업 파일로 현재 데이터를 덮어쓸까요?'))) return;

      const restore = await verifyBackupEnvelope(parsed as any);
      if (!restore.ok && !restore.legacy) {
        toast('복구 중단 · 서명 검증 실패');
        log('restore blocked', restore.code, restore.message);
        return;
      }

      const next = await reverifyStateRecords(restore.state as any);
      next.tab = 'cases';
      next.selectedCaseId = null;
      ui.caseTab = 'list';
      ui.evidenceTab = 'write';

      setState(next);
      await saveState(S);
      syncDraftDefaults();
      render();
      closeDlg('restoreModal');
      resetRestoreFileSelection();
      const trustSummary = next.records.reduce((acc, r) => { const v = verifyRecordIntegrity(r as any) as any; if (!v.valid) acc.warn += 1; else if (v.trusted) acc.verified += 1; else if (v.verificationStatus === 'legacy') acc.legacy += 1; else acc.foreign += 1; return acc; }, { verified: 0, legacy: 0, foreign: 0, warn: 0 });
      toast(`복구 완료 · ${restore.legacy ? '레거시 백업' : '서명확인'} · 검증 ${trustSummary.verified} · 레거시 ${trustSummary.legacy} · 외부 ${trustSummary.foreign} · 경고 ${trustSummary.warn}`);
      log('restore ok', restore.code, trustSummary);
    },

    'open-logs': () => (setText('logBox', logs.join('\n')), openDlg('logsModal')),
    'close-logs': () => closeDlg('logsModal'),
    'copy-logs': async () => (await navigator.clipboard.writeText(logs.join('\n')), toast('로그 복사')),
    'clear-logs': () => (logs.splice(0, logs.length), setText('logBox', ''), toast('로그 비우기')),
    wipe: async () => {
      if (!(await openConfirm('모든 데이터를 삭제할까요?'))) return;
      const prev = JSON.parse(JSON.stringify(S));
      await wipeAll();
      setState(defaultState());
      (S as any).tab = 'cases';
      ui.caseTab = 'records';
      ui.evidenceTab = 'write';
      ui.settingsOpen = false;
      syncDraftDefaults();
      render();
      toastUndo('전체 삭제됨', async () => {
        setState(prev);
        await saveState(S);
        syncDraftDefaults();
        render();
        toast('복구 완료');
      });
      toast('전체 삭제');
      log('wipe all');
    },

    'record-intake': (btn) => {
      const kind = String(btn.dataset.kind || '').trim(); if (!kind) return;
      (draftRecord as any).intake = kind;
      if (kind === '상담') { draftRecord.storeType = '방문상담'; if (draftRecord.place === '기타') draftRecord.place = '교무실'; }
      else if (kind === '관찰') { draftRecord.storeType = '기타'; if (!draftRecord.storeOther) draftRecord.storeOther = '현장관찰'; if (draftRecord.place === '교무실') draftRecord.place = '복도'; }
      else if (kind === '비정형') draftRecord.storeType = '업무메신저';
      else if (kind === '규정') draftRecord.storeType = '문서';
      (draftRecord as any).storeTypeText = draftRecord.storeType; (draftRecord as any).placeText = draftRecord.place; (draftRecord as any).lvText = draftRecord.lv;
      (draftRecord as any).actorTypeText = actorTypeTextFromInternal(draftRecord.actorType); (draftRecord as any).relTypeText = actorTypeTextFromInternal(draftRecord.relType);
      render(); toast(`채널: ${kind}`); log('record intake changed', kind);
    },

    'add-record-actor': () => {
      const actor = makeRelationshipActorRef(String((draftRecord as any).actorGroupId || ''), String((draftRecord as any).actorMemberId || ''));
      if (!actor) return toast('주체 인물을 선택하세요');
      draftRecord.actorNameChoice = OTHER;
      draftRecord.actors = addActorToList((draftRecord as any).actors || [], actor);
      (draftRecord as any).actorMemberId = '';
      render();
      toast('주체 추가');
    },
    'add-record-actor-edit': () => {
      const typeText = normalizeActorTypeTextUI(String((draftRecordEdit as any).actorTypeText || '').trim());
      const type = actorTypeInternalFromText(typeText);
      draftRecordEdit.actorType = type;
      (draftRecordEdit as any).actorTypeText = preserveActorTypeText(typeText, type);
      const name = String(draftRecordEdit.actorNameOther || '').trim();
      if (!typeText || !name) return toast('주체 정보를 입력하세요');
      draftRecordEdit.actorNameChoice = OTHER;
      draftRecordEdit.actors = addActorToList((draftRecordEdit as any).actors || [], { type, name });
      draftRecordEdit.actorNameOther = '';
      render();
      toast('주체 추가');
    },
    'remove-record-actor': (btn) => {
      const idx = Number(btn.dataset.idx ?? '-1');
      if (!Number.isNaN(idx) && idx >= 0) ((draftRecord as any).actors = ((draftRecord as any).actors || []).filter((_: any, i: number) => i !== idx), render());
    },
    'remove-record-actor-edit': (btn) => {
      const idx = Number(btn.dataset.idx ?? '-1');
      if (!Number.isNaN(idx) && idx >= 0) ((draftRecordEdit as any).actors = ((draftRecordEdit as any).actors || []).filter((_: any, i: number) => i !== idx), render());
    },

    'add-related': () => {
      const actor = makeRelationshipActorRef(String((draftRecord as any).relGroupId || ''), String((draftRecord as any).relMemberId || ''));
      if (!actor) return toast('관련 인물을 선택하세요');
      draftRecord.relNameChoice = OTHER;
      draftRecord.related = addActorToList(draftRecord.related || [], actor);
      (draftRecord as any).relMemberId = '';
      ui.recRelatedOpen = true;
      render(); toast('관련자 추가'); log('related added', actor.name);
    },
    'add-related-edit': () => {
      const typeText = normalizeActorTypeTextUI(String((draftRecordEdit as any).relTypeText || '').trim());
      const type = actorTypeInternalFromText(typeText);
      draftRecordEdit.relType = type; (draftRecordEdit as any).relTypeText = preserveActorTypeText(typeText, type);
      const name = String(draftRecordEdit.relNameOther || '').trim();
      if (!typeText || !name) return;
      draftRecordEdit.relNameChoice = OTHER;
      draftRecordEdit.related = addActorToList(draftRecordEdit.related || [], { type, name });
      draftRecordEdit.relNameOther = '';
      ui.recEditRelatedOpen = true;
      render(); toast('관련자 추가'); log('related added(edit)', name);
    },
    'remove-related': (btn) => { const idx = Number(btn.dataset.idx ?? '-1'); if (!Number.isNaN(idx) && idx >= 0) (draftRecord.related = (draftRecord.related || []).filter((_, i) => i !== idx), render()); },
    'remove-related-edit': (btn) => { const idx = Number(btn.dataset.idx ?? '-1'); if (!Number.isNaN(idx) && idx >= 0) (draftRecordEdit.related = (draftRecordEdit.related || []).filter((_, i) => i !== idx), render()); },
    'clear-record-draft': () => (Object.assign(draftRecord, DEFAULT_RECORD()), render()),

    'set-record-now': () => { draftRecord.ts = toLocalInputValue(nowISO()); render(); toast('기록 시각: 지금'); log('record ts set now'); },
    'set-record-edit-now': () => { draftRecordEdit.ts = toLocalInputValue(nowISO()); render(); toast('기록 시각: 지금'); log('record edit ts set now'); },
    'start-edit-record': (btn) => {
      const id = String(btn.dataset.id || '').trim();
      if (!id) return;
      const r = S.records.find((x) => x.id === id);
      if (!r) return;
      ui.viewRecordId = id;
      ui.recordEditId = id;
      ui.recordModalTab = 'edit';
      ui.recEditRelatedOpen = false;
      loadRecordEditDraft(r);
      render();
      openRecordModal();
      log('record edit open', id);
    },
    'cancel-record-edit': () => {
      ui.recordEditId = null;
      ui.recordModalTab = 'current';
      ui.recEditRelatedOpen = false;
      resetRecordEditDraft();
      render();
      openRecordModal();
      log('record edit cancel');
    },

    'save-record': async () => {
      const prep = prepareRecordDraftForSeal() as any;
      if (prep.error) return toast(prep.error);
      (draftRecord as any).signerLabel = String((draftRecord as any).signerLabel || '').trim() || '기기 봉인서명';
      (draftRecord as any).sealReason = String((draftRecord as any).sealReason || '').trim() || '초기 기록 봉인';
      openSignatureModal('create');
    },

    'save-record-amend': async () => {
      const id = String(ui.recordEditId || '').trim();
      if (!id) return toast('수정할 기록을 찾을 수 없어요');
      const idx = S.records.findIndex((x) => x.id === id);
      if (idx < 0) return toast('수정할 기록을 찾을 수 없어요');
      const prep = prepareRecordEditForSeal() as any;
      if (prep.error) return toast(prep.error);
      (draftRecordEdit as any).signerLabel = String((draftRecordEdit as any).signerLabel || '').trim() || '기기 봉인서명';
      (draftRecordEdit as any).sealReason = String((draftRecordEdit as any).sealReason || '').trim() || '기록 정정 및 재봉인';
      openSignatureModal('amend');
    },

    'view-record': (btn) => { const id = btn.dataset.id; if (!id) return; ui.viewRecordId = id; ui.recordEditId = null; ui.recordModalTab = 'current'; ui.recEditRelatedOpen = false; resetRecordEditDraft(); render(); openRecordModal(); log('record view', id); },
    'view-timeline': (btn) => { const id = btn.dataset.id; const kind = (btn.dataset.kind as any) || 'record'; if (!id) return; if (kind !== 'record' && kind !== 'advisor' && kind !== 'step') return; ui.viewTimelineItem = { kind, id }; render(); openTimelineModal(); log('timeline view', { kind, id }); },

    'delete-record': async (btn) => {
      const id = btn.dataset.id; if (!id) return;
      const r = S.records.find((x) => x.id === id); if (!r) return;
      const holders = casesContainingRecord(r, S.cases);
      if (holders.length) return void (toast(`기록묶음 ${holders.length}개에 포함된 기록이라 삭제할 수 없어요.`), log('delete-record blocked (in cases)', id));
      if (!(await openConfirm('이 기록을 삭제할까요?'))) return;
      S.records = S.records.filter((x) => x.id !== id); await SR();
      toastUndo('기록 삭제됨', async () => (S.records.unshift(r), await SR(), toast('복구 완료')));
      log('record deleted', id);
    },

    'remove-record-from-case': async (btn) => {
      const c = mustCase(); if (!c) return;
      const id = btn.dataset.id; if (!id) return;
      if (!(await openConfirm('이 기록을 이 기록묶음에서 뺄까요? (기록 자체가 삭제되진 않아요)'))) return;
      
      const prevIds = (c.recordIds || []).slice();
      c.recordIds = prevIds.filter((x) => x !== id);
      await SR();
      toastUndo('묶음에서 제외됨', async () => { c.recordIds = prevIds; await SR(); toast('복구 완료'); });
      log('removed from case', id);
    },

    'copy-record': async (btn) => {
      const id = btn.dataset.id; if (!id) return;
      const r = S.records.find((x) => x.id === id); if (!r) return;
      await navigator.clipboard.writeText(JSON.stringify(ensureRecordV8(r), null, 2));
      toast('복사'); log('record copied', id);
    },

    'add-case-main-actor': () => {
      const actor = parseActorChoice(String((draftCase as any).mainActorKey || ''));
      if (!actor) return toast('기록 보관함 주체를 선택하세요');
      draftCase.actors = addActorToList(draftCase.actors || [], actor);
      (draftCase as any).mainActorKey = '';
      render();
      toast('주체 추가');
    },
    'add-case-related-actor': () => {
      const actor = parseActorChoice(String((draftCase as any).relatedActorKey || ''));
      if (!actor) return toast('기록 보관함 관련자를 선택하세요');
      draftCase.actors = addActorToList(draftCase.actors || [], actor);
      (draftCase as any).relatedActorKey = '';
      render();
      toast('관련자 추가');
    },
    'remove-case-actor': (btn) => { const idx = Number(btn.dataset.idx ?? '-1'); if (!Number.isNaN(idx) && idx >= 0) (draftCase.actors = (draftCase.actors || []).filter((_, i) => i !== idx), render()); },
    'clear-case-draft': () => (Object.assign(draftCase, DEFAULT_CASE()), render()),

    'create-case': async () => {
      if (!(draftCase.actors || []).length) return toast('대상을 1명 이상 추가한 뒤 시작할 수 있어요');

      // ✅ [제목 자동 생성 로직]
      let title = String(draftCase.title || '').trim();
      const query = String(draftCase.query || '').trim();

      if (!title) {
        // 제목이 비어있으면 "{주체} {요약(키워드)} 기록묶음" 포맷으로 생성
        const mainActor = draftCase.actors[0];
        const actorName = mainActor ? actorShort(mainActor) : '미정';
        
        // 요약이 너무 길면 잘라서 사용
        const shortQuery = query.length > 12 ? query.slice(0, 12) + '...' : query;
        
        title = `${actorName} ${shortQuery} 기록묶음`.replace(/\s+/g, ' ').trim();
      }

      const { caseItem, error, pickedCount } = await createCaseWithAdvisors({
        title, 
        actors: (draftCase.actors || []).slice(), 
        query,
        timeFromISO: draftCase.timeFrom ? fromLocalInputValue(draftCase.timeFrom) : '', 
        timeToISO: draftCase.timeTo ? fromLocalInputValue(draftCase.timeTo) : '',
        sensFilter: 'any' as any, 
        status: '진행중' as any, 
        maxResults: draftCase.maxResults,
        onlyMainActor: !!(draftCase as any).onlyMainActor
      }, S.records, () => uid('CASE'), nowISO);

      if (error) return toast(error);
      const c = caseItem!; S.cases[c.id] = c; S.selectedCaseId = c.id; S.tab = 'cases'; ui.caseTab = 'list';
      Object.assign(draftCase, DEFAULT_CASE()); await SR(); closeCaseCreateModal(); render();
      setText('caseCreatedMsg', `“${String(c.title || '').trim() || '기록묶음'}” 생성됨`);
      setText('caseCreatedSub', pickedCount ? `AI가 기록 ${pickedCount}개를 모았어요.` : 'AI가 포함할 기록을 찾지 못했어요.');
      openDlg('caseCreatedModal'); window.setTimeout(() => closeDlg('caseCreatedModal'), 2000);
      toast('생성 완료 ✅'); log('case created', c.id);
    },

    'select-case': async (btn) => { const id = btn.dataset.id; if (!id || !S.cases[id]) return; S.selectedCaseId = id; S.tab = 'cases'; ui.caseTab = 'list'; await SR(); log('case selected', id); },
    'clear-case': async () => { S.selectedCaseId = null; ui.qTimeline = ''; ui.caseTab = 'list'; await SR(); log('case cleared'); },

    'open-paper-picker': () => { if (!Object.keys(S.cases || {}).length) return toast('먼저 기록묶음을 만들어주세요'); S.tab = 'cases' as any; ui.caseTab = 'proof' as any; ui.paperPickOpen = false; ui.paperPickQuery = ''; render(); void saveState(S); log('content proof section open'); },
    'close-paper-picker': () => (closePaperPickModal(), render(), log('paper picker close')),
    'pick-paper-case': async (btn) => { const id = String(btn.dataset.id || '').trim(); const c = id ? (S.cases[id] ?? null) : null; if (!c) return; ui.paperCaseId = c.id; ui.paperHash = await computeCasePaperHash(c); ensureContentProofDraft(); closePaperPickModal(); render(); openPaperModal(); log('content proof open (picker)', c.id); },
    'paper-open-case-create': () => { closePaperPickModal(); S.tab = 'cases' as any; ui.caseTab = 'list'; ui.caseCreateOpen = true; render(); openCaseCreateModal(); void saveState(S); log('case create modal open (from paper picker)'); },

    'open-paper': async () => { const c = mustCase(); if (!c) return; ui.paperCaseId = c.id; ui.caseTab = 'proof' as any; S.tab = 'cases' as any; render(); log('content proof tab open', c.id); },
    'pick-proof-case': async (btn) => { const id = String(btn.dataset.id || '').trim(); if (!id || !S.cases[id]) return; S.selectedCaseId = id; ui.paperCaseId = id; ui.caseTab = 'proof' as any; await saveState(S); render(); log('proof case picked', id); },
    'open-paper-preview': async () => {
      const c = (ui.paperCaseId && S.cases[ui.paperCaseId]) ? S.cases[ui.paperCaseId] : null;
      if (!c) return toast('먼저 기록묶음 목록에서 항목을 선택하세요');
      ui.paperCaseId = c.id;
      ui.paperHash = await computeCasePaperHash(c);
      ensureContentProofDraft();
      render();
      openPaperModal();
      log('content proof preview open', c.id);
    },
    'close-paper': () => (closePaperModal(), render()),
    'print-paper': async () => {
      const c = ui.paperCaseId ? S.cases[ui.paperCaseId] ?? null : null; if (!c) return;
      try {
        const proof = ensureContentProofDraft();
        if (!String(proof.senderName || '').trim() || !String(proof.senderAddress || '').trim() || !String(proof.recipientName || '').trim() || !String(proof.recipientAddress || '').trim()) {
          return toast('발신인/수신인 이름과 주소를 모두 입력해주세요');
        }
        const suggested = `${c.title}__공유문서.pdf`.replace(/\s+/g, ' ').trim();
        const path = await saveDialog({ defaultPath: suggested, filters: [{ name: 'PDF', extensions: ['pdf'] }] });
        if (!path) return toast('저장 취소됨');
        const generatedAt = nowISO();
        const recs = recordsForCase(S.records, c);
        const { events } = buildCaseTimeline(c, S.records, '');
        const payload = buildPaperPayload(c, recs, events, generatedAt, ui.paperHash, proof);
        const savedPath = await invoke<string>('export_case_pdf', { args: { paper: payload, fileName: path } });
        toast('공유 문서 PDF 저장 완료'); log('content proof pdf exported', savedPath);
      } catch (e: any) { console.error(e); toast(`PDF 저장 실패: ${String(e?.message || e)}`); }
    },

    'open-case-update': () => { const c = mustCase(); if (c) (openUpdate(c.id), log('case update modal open', c.id)); },
    'close-case-update': () => (closeCaseUpdateModal(), render()),
    'apply-case-update': async () => {
      const c = ui.updateCaseId ? S.cases[ui.updateCaseId] ?? null : null; if (!c) return toast('기록묶음을 찾을 수 없어요');
      const ids = (ui.updatePickIds || []).slice();
      // fallback (혹시 state가 비어있을 때)
      if (!ids.length) {
        const checked = Array.from(dlg('caseUpdateModal')?.querySelectorAll<HTMLInputElement>('input[name="caseUpdPick"]:checked') || []);
        ids.push(...checked.map((x) => x.value).filter(Boolean));
      }
      if (!ids.length) return toast('선택된 항목이 없어요');
      S.cases[c.id] = await addRecordsToCase(c, S.records, ids);
      await SR(); closeCaseUpdateModal(); render(); toast(`${ids.length}개 기록 추가됨`); log('case records added', c.id, ids.length);
    },
    'delete-case': async (btn) => {
      const id = btn.dataset.id; if (!id || !S.cases[id]) return;
      if (!(await openConfirm('이 기록묶음을 삭제할까요?'))) return;
      const deleted = S.cases[id]; delete S.cases[id]; if (S.selectedCaseId === id) S.selectedCaseId = null;
      await SR(); toastUndo('기록묶음 삭제됨', async () => (S.cases[deleted.id] = deleted, await SR(), toast('복구 완료'))); log('case deleted', id);
    },

    'add-step': async () => {
      const c = mustCase(); if (!c) return;
      const name = draftStep.name.trim(), note = draftStep.note.trim();
      if (!name || !note) return toast('단계/내용은 필수예요');
      const step: StepItem = { id: uid('STEP'), ts: fromLocalInputValue(draftStep.ts), name, note, text: '', place: '', owner: '', lv: '' };
      c.steps = Array.isArray(c.steps) ? c.steps : []; c.steps.push(step);
      draftStep.name = ''; draftStep.note = ''; flash(step.id); await SR(); toast('내 조치 로그 추가됨'); log('step added', step.id);
    },
    'delete-step': async (btn) => {
      const c = mustCase(); const id = btn.dataset.id; if (!c || !id) return;
      if (!(await openConfirm('이 단계를 삭제할까요?'))) return;
      const deleted = (c.steps || []).find((s) => s.id === id); c.steps = (c.steps || []).filter((s) => s.id !== id);
      await SR(); toastUndo('단계 삭제됨', async () => { if (!deleted) return; c.steps = Array.isArray(c.steps) ? c.steps : []; c.steps.push(deleted); await SR(); toast('복구 완료'); }); log('step deleted', id);
    },
    'regen-advisors': async () => {
      const c = mustCase(); if (!c) return;
      if (!(await openConfirm('대응 가이드를 현재 규칙으로 다시 생성할까요? (숨긴 대응 가이드은 사라져요)'))) return;
      c.advisors = await regenerateCaseAdvisors(c, S.records); await SR(); toast('대응 가이드 재생성됨'); log('advisors regenerated', c.id);
    },
    'toggle-advisor-done': async (btn) => {
      const c = mustCase(); const id = btn.dataset.id; if (!c || !id) return;
      c.advisors = Array.isArray(c.advisors) ? c.advisors : []; const a = c.advisors.find((x) => x.id === id); if (!a) return;
      a.state = a.state === 'done' ? 'active' : 'done'; await SR(); toast(a.state === 'done' ? '완료 처리' : '다시 열기'); log('advisor toggled', id);
    },
    'dismiss-advisor': async (btn) => {
      const c = mustCase(); const id = btn.dataset.id; if (!c || !id) return;
      c.advisors = Array.isArray(c.advisors) ? c.advisors : []; const a = c.advisors.find((x) => x.id === id); if (!a) return;
      const prev = a.state; a.state = 'dismissed'; await SR(); toastUndo('대응 가이드 숨김', async () => (a.state = prev, await SR(), toast('복구 완료'))); log('advisor dismissed', id);
    },
    'advisor-to-step': async (btn) => {
      const c = mustCase(); const id = btn.dataset.id; if (!c || !id) return;
      c.advisors = Array.isArray(c.advisors) ? c.advisors : []; const a = c.advisors.find((x) => x.id === id); if (!a) return;
      const step: StepItem = { id: uid('STEP'), ts: String(a.ts || nowISO()), name: `대응 가이드: ${a.title}`.slice(0, 60), note: a.body, text: '', place: '', owner: '', lv: '' };
      c.steps = Array.isArray(c.steps) ? c.steps : []; c.steps.push(step); a.state = 'done'; flash(step.id); await SR(); toast('내 조치 로그로 저장됨'); log('advisor -> step', `${id} -> ${step.id}`);
    },
  };

  document.addEventListener('click', async (e) => {
    const btn = (e.target as HTMLElement | null)?.closest<HTMLElement>('[data-action]');
    const action = btn?.dataset.action; if (!btn || !action) return;
    try { const fn = click[action]; if (fn) await fn(btn); } catch (err) { toast('오류 발생: 로그를 확인하세요'); log('ERROR', err); }
  });

  document.addEventListener('click', (e) => {
    const target = e.target as HTMLElement | null;
    if (!target) return;
    if (target.closest('.strategyModelPicker')) return;
    if (!(ui as any).strategyChatModelMenuOpen) return;
    closeStrategyChatModelMenu();
    render();
  });



  /* ---------- restore: file upload ---------- */
  const setRestoreLabel = () => {
    const info = document.getElementById('restoreFileName');
    if (info) info.textContent = _restoreFileName ? `선택됨: ${_restoreFileName}` : '선택된 파일 없음';
  };

  const readRestoreFile = (file: File) => {
    _restoreFileName = file.name || 'backup.json';
    setRestoreLabel();

    const reader = new FileReader();
    reader.onload = () => {
      _restoreFileText = String(reader.result || '');
      setRestoreLabel();
      toast('백업 파일 선택됨');
      log('restore file loaded', _restoreFileName || '');
    };
    reader.onerror = () => {
      _restoreFileText = null;
      toast('파일을 읽지 못했어요');
      log('restore file read error', _restoreFileName || '');
    };
    reader.readAsText(file, 'utf-8');
  };

  // file input change
  document.addEventListener('change', (e) => {
    const input = e.target as HTMLInputElement | null;
    if (!input || input.id !== 'restoreFile') return;
    const file = input.files?.[0];
    if (!file) return;
    readRestoreFile(file);
  });

  // drag & drop on drop zone
  const dzOf = (t: EventTarget | null) =>
    (t as HTMLElement | null)?.closest<HTMLElement>('#restoreDropZone');

  document.addEventListener('dragover', (e) => {
    const dz = dzOf(e.target);
    if (!dz) return;
    e.preventDefault();
    dz.classList.add('isDrag');
  });

  document.addEventListener('dragleave', (e) => {
    const dz = dzOf(e.target);
    if (!dz) return;
    dz.classList.remove('isDrag');
  });

  document.addEventListener('drop', (e) => {
    const dz = dzOf(e.target);
    if (!dz) return;
    e.preventDefault();
    dz.classList.remove('isDrag');
    const file = (e as DragEvent).dataTransfer?.files?.[0];
    if (file) readRestoreFile(file);
  });

  /* ---------- input/change routing ---------- */
  const rec: Record<string, (v: string) => void> = {
    actorGroupId: (v) => {
      (draftRecord as any).actorGroupId = v;
      (draftRecord as any).actorMemberId = '';
      render();
    },
    actorMemberId: (v) => ((draftRecord as any).actorMemberId = v),
    relGroupId: (v) => {
      (draftRecord as any).relGroupId = v;
      (draftRecord as any).relMemberId = '';
      ui.recRelatedOpen = true;
      render();
    },
    relMemberId: (v) => ((draftRecord as any).relMemberId = v),
    actorTypeText: (v) => {
      const prevText = String((draftRecord as any).actorTypeText || '');
      const uiText = normalizeActorTypeTextUI(v);
      const t = actorTypeInternalFromText(uiText);
      draftRecord.actorType = t;
      (draftRecord as any).actorTypeText = preserveActorTypeText(uiText, t);
      if (didActorTypePickerChange(prevText, uiText)) {
        draftRecord.actorNameChoice = OTHER;
        draftRecord.actorNameOther = '';
      }
      render();
    },
    actorNameOther: (v) => (draftRecord.actorNameChoice = OTHER, draftRecord.actorNameOther = v),
    relTypeText: (v) => {
      const prevText = String((draftRecord as any).relTypeText || '');
      const uiText = normalizeActorTypeTextUI(v);
      const t = actorTypeInternalFromText(uiText);
      draftRecord.relType = t;
      (draftRecord as any).relTypeText = preserveActorTypeText(uiText, t);
      if (didActorTypePickerChange(prevText, uiText)) { draftRecord.relNameChoice = OTHER; draftRecord.relNameOther = ''; }
      ui.recRelatedOpen = true;
      render();
    },
    relNameOther: (v) => (draftRecord.relNameChoice = OTHER, draftRecord.relNameOther = v),
    placeText: (v) => { (draftRecord as any).placeText = v; draftRecord.place = (PLACE_TYPES as any).includes(v as any) ? (v as PlaceType) : ('기타' as PlaceType); if (draftRecord.place !== '기타') draftRecord.placeOther = ''; render(); },
    placeOther: (v) => (draftRecord.placeOther = v),
    storeTypeText: (v) => { (draftRecord as any).storeTypeText = v; draftRecord.storeType = (STORE_TYPES as any).includes(v as any) ? (v as StoreType) : ('기타' as StoreType); if (draftRecord.storeType !== '기타') draftRecord.storeOther = ''; render(); },
    storeOther: (v) => (draftRecord.storeOther = v),
    lvText: (v) => ((draftRecord as any).lvText = v, (LVS as any).includes(v as any) && (draftRecord.lv = v as Sensitivity)),
    ts: (v) => (draftRecord.ts = v),
    summary: (v) => (draftRecord.summary = v),
    summaryOverview: (v) => ((draftRecord as any).summaryOverview = v),
    summaryBackground: (v) => ((draftRecord as any).summaryBackground = v),
    summaryIssues: (v) => ((draftRecord as any).summaryIssues = v),
    summaryEvidenceList: (v) => ((draftRecord as any).summaryEvidenceList = v),
    summaryTeacherActions: (v) => ((draftRecord as any).summaryTeacherActions = v),
    summaryOther: (v) => ((draftRecord as any).summaryOther = v),
    signerLabel: (v) => ((draftRecord as any).signerLabel = v),
    sealReason: (v) => ((draftRecord as any).sealReason = v),
  };

  const recEdit: Record<string, (v: string) => void> = {
    actorTypeText: (v) => {
      const prevText = String((draftRecordEdit as any).actorTypeText || '');
      const uiText = normalizeActorTypeTextUI(v);
      const t = actorTypeInternalFromText(uiText);
      draftRecordEdit.actorType = t;
      (draftRecordEdit as any).actorTypeText = preserveActorTypeText(uiText, t);
      if (didActorTypePickerChange(prevText, uiText)) {
        draftRecordEdit.actorNameChoice = OTHER;
        draftRecordEdit.actorNameOther = '';
      }
      render();
    },
    actorNameOther: (v) => (draftRecordEdit.actorNameChoice = OTHER, draftRecordEdit.actorNameOther = v),
    relTypeText: (v) => {
      const prevText = String((draftRecordEdit as any).relTypeText || '');
      const uiText = normalizeActorTypeTextUI(v);
      const t = actorTypeInternalFromText(uiText);
      draftRecordEdit.relType = t;
      (draftRecordEdit as any).relTypeText = preserveActorTypeText(uiText, t);
      if (didActorTypePickerChange(prevText, uiText)) { draftRecordEdit.relNameChoice = OTHER; draftRecordEdit.relNameOther = ''; }
      ui.recEditRelatedOpen = true;
      render();
    },
    relNameOther: (v) => (draftRecordEdit.relNameChoice = OTHER, draftRecordEdit.relNameOther = v),
    placeText: (v) => { (draftRecordEdit as any).placeText = v; draftRecordEdit.place = (PLACE_TYPES as any).includes(v as any) ? (v as PlaceType) : ('기타' as PlaceType); if (draftRecordEdit.place !== '기타') draftRecordEdit.placeOther = ''; render(); },
    placeOther: (v) => (draftRecordEdit.placeOther = v),
    storeTypeText: (v) => { (draftRecordEdit as any).storeTypeText = v; draftRecordEdit.storeType = (STORE_TYPES as any).includes(v as any) ? (v as StoreType) : ('기타' as StoreType); if (draftRecordEdit.storeType !== '기타') draftRecordEdit.storeOther = ''; render(); },
    storeOther: (v) => (draftRecordEdit.storeOther = v),
    lvText: (v) => ((draftRecordEdit as any).lvText = v, (LVS as any).includes(v as any) && (draftRecordEdit.lv = v as Sensitivity)),
    ts: (v) => (draftRecordEdit.ts = v),
    summary: (v) => (draftRecordEdit.summary = v),
    summaryOverview: (v) => ((draftRecordEdit as any).summaryOverview = v),
    summaryBackground: (v) => ((draftRecordEdit as any).summaryBackground = v),
    summaryIssues: (v) => ((draftRecordEdit as any).summaryIssues = v),
    summaryEvidenceList: (v) => ((draftRecordEdit as any).summaryEvidenceList = v),
    summaryTeacherActions: (v) => ((draftRecordEdit as any).summaryTeacherActions = v),
    summaryOther: (v) => ((draftRecordEdit as any).summaryOther = v),
    signerLabel: (v) => ((draftRecordEdit as any).signerLabel = v),
    sealReason: (v) => ((draftRecordEdit as any).sealReason = v),
  };

  const cas: Record<string, (v: string) => void> = {
    title: (v) => (draftCase.title = v), query: (v) => (draftCase.query = v), timeFrom: (v) => (draftCase.timeFrom = v), timeTo: (v) => (draftCase.timeTo = v),
    maxResults: (v) => (draftCase.maxResults = Math.max(1, Math.min(400, Number(v) || 80))),
    sensFilterText: (v) => { (draftCase as any).sensFilterText = v; const vv = String(v || '').trim(); if (vv === 'any' || vv === '전체') draftCase.sensFilter = 'any'; else if ((LVS as any).includes(vv as any)) draftCase.sensFilter = vv as any; },
    statusText: (v) => { (draftCase as any).statusText = v; const vv = String(v || '').trim(); (STATUSES as any).includes(vv as any) && (draftCase.status = vv as any); },
    mainActorKey: (v) => ((draftCase as any).mainActorKey = v),
    relatedActorKey: (v) => ((draftCase as any).relatedActorKey = v),
    onlyMainActor: (v) => ((draftCase as any).onlyMainActor = (v === 'true')),
  };

  const step: Record<string, (v: string) => void> = { ts: (v) => (draftStep.ts = v), name: (v) => (draftStep.name = v), note: (v) => (draftStep.note = v) };

  const handle = (el: HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement) => {
    const action = el.dataset.action, field = el.dataset.field; if (!action || !field) return;
    const v = (el instanceof HTMLInputElement && el.type === 'checkbox') ? (el.checked ? 'true' : 'false') : el.value;
    if (action === 'draft-screen-pin') {
      const next = normalizeScreenPin(v);
      if (field === 'pin') ui.pinEntryDraft = next;
      else if (field === 'confirm') ui.pinConfirmDraft = next;
      return;
    }
    if (action === 'draft-pin-settings') {
      const next = normalizeScreenPin(v);
      if (field === 'pin') ui.pinSettingsDraft = next;
      else if (field === 'confirm') ui.pinSettingsConfirmDraft = next;
      return;
    }
    if (action === 'draft-strategy-backend-settings') {
      const draft = (((ui as any).strategyLlamaServerConfig || defaultStrategyLlamaServerConfig()) as StrategyLlamaServerConfigDraft);
      if (field === 'mode') {
        (ui as any).strategyBackendMode = 'llama-server';
      } else if (field === 'hyperclovaUrl') {
        draft.hyperclovaUrl = v;
      } else if (field === 'roosyUrl') {
        draft.roosyUrl = v;
      } else if (field === 'hyperclovaSlot') {
        draft.hyperclovaSlot = String(v || '').replace(/[^\d]/g, '');
      } else if (field === 'roosySlot') {
        draft.roosySlot = String(v || '').replace(/[^\d]/g, '');
      } else if (field === 'startupTimeoutMs') {
        draft.startupTimeoutMs = String(v || '').replace(/[^\d]/g, '');
      } else if (field === 'requestTimeoutMs') {
        draft.requestTimeoutMs = String(v || '').replace(/[^\d]/g, '');
      } else if (field === 'cachePrompt') {
        draft.cachePrompt = v === 'true';
      }
      (ui as any).strategyLlamaServerConfig = draft;
      render();
      return;
    }
    if (action === 'draft-record-filters') {
      if (field === 'actor') ui.recFilterActorDraft = v;
      else if (field === 'place') ui.recFilterPlaceDraft = v;
      else if (field === 'keyword') ui.recFilterKeywordDraft = v;
      return;
    }
    if (action === 'draft-update-filters') {
      if (field === 'actor') ui.updFilterActorDraft = v;
      else if (field === 'place') ui.updFilterPlaceDraft = v;
      else if (field === 'keyword') ui.updFilterKeywordDraft = v;
      return;
    }
    if (action === 'toggle-update-pick') {
      const id = String((el as HTMLInputElement).value || '').trim();
      if (!id) return;
      const arr = ui.updatePickIds || (ui.updatePickIds = []);
      const has = arr.includes(id);
      const want = (el as HTMLInputElement).checked;
      if (want && !has) arr.push(id);
      if (!want && has) ui.updatePickIds = arr.filter((x) => x !== id);
      return;
    }
    if (action === 'search-timeline') return void (ui.qTimeline = v, render());
    if (action === 'search-paper-cases') return void (ui.paperPickQuery = v, render());
    if (action === 'draft-content-proof') {
      const draft = ensureContentProofDraft();
      if (field === 'senderName') draft.senderName = v;
      else if (field === 'senderAddress') draft.senderAddress = v;
      else if (field === 'recipientName') draft.recipientName = v;
      else if (field === 'recipientAddress') draft.recipientAddress = v;
      updateContentProofUI();
      return;
    }
    if (action === 'draft-strategy-chat') {
      const prevMentionOpen = !!(ui as any).strategyMentionOpen;
      const prevMentionKey = strategyMentionSuggestionsKey(
        Array.isArray((ui as any).strategyMentionSuggestions)
          ? ((ui as any).strategyMentionSuggestions as StrategyMentionSuggestion[])
          : []
      );
      (ui as any).strategyChatInput = v;
      if (el instanceof HTMLTextAreaElement) {
        syncStrategyMentionState(v, el.selectionStart ?? v.length);
        const nextMentionOpen = !!(ui as any).strategyMentionOpen;
        const nextMentionKey = strategyMentionSuggestionsKey(
          Array.isArray((ui as any).strategyMentionSuggestions)
            ? ((ui as any).strategyMentionSuggestions as StrategyMentionSuggestion[])
            : []
        );
        if (
          prevMentionOpen !== nextMentionOpen
          || prevMentionKey !== nextMentionKey
        ) {
          render();
        }
      }
      return;
    }
    if (action === 'draft-strategy-record-edit') {
      (ui as any).strategyRecordEditDraft = v;
      return;
    }
    if (action === 'draft-simulation') {
      const draft = ensureSimulationDraft();
      if (field === 'evidenceFilter') {
        draft.evidenceFilter = v;
        scheduleSimulationSearchRender();
        return;
      }
      if (field === 'pickerQuery') {
        (ui as any).simulationPickerQuery = v;
        scheduleSimulationSearchRender();
        return;
      }
      if (field === 'caseId') {
        const nextCaseId = String(v || '').trim();
        if (nextCaseId && S.cases[nextCaseId]) {
          applySimulationCaseSelection(nextCaseId);
          if ((ui as any).simulationPickerOpen) {
            setSimulationPickerSelectedIds(getSimulationCaseRecordIds(nextCaseId));
          }
          toast(`컬렉션 기록 ${getSimulationCaseRecordIds(nextCaseId).length}개를 기본으로 불러왔어요`);
        } else {
          (ui as any).simulationCaseId = null;
          (ui as any).simulationResult = null;
          markSimulationDirty();
        }
        render();
        return;
      }
      if (field === 'goal') {
        draft.goal = (['stabilize', 'document', 'escalate'].includes(String(v || '')) ? v : 'stabilize') as SimulationDraft['goal'];
      } else if (field === 'parentTone' || field === 'escalation' || field === 'persistence' || field === 'adminSupport' || field === 'publicSpread' || field === 'legalLeverage') {
        (draft as any)[field] = clampSimulationNumber(v, (SIMULATION_DEFAULTS as any)[field] ?? 50);
      }
      (ui as any).simulationResult = null;
      markSimulationDirty();
      render();
      return;
    }
    if (action === 'search-update-candidates') return void (ui.qUpdate = v, render()); 
    const table = action === 'draft-record' ? rec : action === 'draft-record-edit' ? recEdit : action === 'draft-case' ? cas : action === 'draft-step' ? step : null;
    table?.[field]?.(v);
    if (action === 'draft-record') updateRecordComposerUI();
    if (action === 'draft-record-edit') updateRecordEditUI();
  };

  const watch = '[data-action="draft-record"],[data-action="draft-record-edit"],[data-action="draft-case"],[data-action="draft-step"],[data-action="draft-record-filters"],[data-action="draft-update-filters"],[data-action="toggle-update-pick"],[data-action="search-timeline"],[data-action="search-paper-cases"],[data-action="search-update-candidates"],[data-action="draft-screen-pin"],[data-action="draft-pin-settings"],[data-action="draft-strategy-backend-settings"],[data-action="draft-content-proof"],[data-action="draft-simulation"],[data-action="draft-strategy-chat"],[data-action="draft-strategy-action"],[data-action="draft-strategy-record-edit"]';
  const isSimulationImeField = (el: HTMLElement | null) => !!el && el.dataset.action === 'draft-simulation' && (el.dataset.field === 'evidenceFilter' || el.dataset.field === 'pickerQuery');
  const isStrategyChatImeField = (el: HTMLElement | null) => !!el && el.dataset.action === 'draft-strategy-chat' && el.dataset.field === 'input';
  document.addEventListener('input', (e) => {
    const el = (e.target as HTMLElement | null)?.closest<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>(watch);
    if (!el) return;
    if (isSimulationImeField(el) && 'isComposing' in e && (e as InputEvent).isComposing) return;
    if (isStrategyChatImeField(el) && 'isComposing' in e && (e as InputEvent).isComposing) {
      (ui as any).strategyChatInput = (el as HTMLTextAreaElement).value;
      return;
    }
    handle(el);
  });
  document.addEventListener('compositionend', (e) => {
    const el = (e.target as HTMLElement | null)?.closest<HTMLInputElement | HTMLTextAreaElement>('[data-action="draft-simulation"][data-field="evidenceFilter"],[data-action="draft-simulation"][data-field="pickerQuery"]');
    if (el) handle(el);
  });
  document.addEventListener('compositionstart', (e) => {
    const el = (e.target as HTMLElement | null)?.closest<HTMLTextAreaElement>('[data-action="draft-strategy-chat"][data-field="input"]');
    if (!el) return;
    el.closest('.strategyComposerTextareaWrapOnly')?.classList.add('isComposing');
  });
  document.addEventListener('compositionend', (e) => {
    const el = (e.target as HTMLElement | null)?.closest<HTMLTextAreaElement>('[data-action="draft-strategy-chat"][data-field="input"]');
    if (!el) return;
    el.closest('.strategyComposerTextareaWrapOnly')?.classList.remove('isComposing');
    handle(el);
  });
  document.addEventListener('change', (e) => { const el = (e.target as HTMLElement | null)?.closest<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>('[data-action="draft-record"],[data-action="draft-record-edit"],[data-action="draft-case"],[data-action="draft-step"],[data-action="draft-record-filters"],[data-action="draft-update-filters"],[data-action="toggle-update-pick"],[data-action="draft-screen-pin"],[data-action="draft-pin-settings"],[data-action="draft-strategy-backend-settings"],[data-action="draft-content-proof"],[data-action="draft-simulation"],[data-action="draft-strategy-chat"],[data-action="draft-strategy-action"],[data-action="draft-strategy-record-edit"]'); el && handle(el); });

  document.addEventListener('keydown', (e) => {
    const el = (e.target as HTMLElement | null)?.closest<HTMLTextAreaElement>('[data-action="draft-strategy-chat"][data-field="input"]');
    if (!el) return;
    if (!('isComposing' in e) || !(e as KeyboardEvent).isComposing) {
      syncStrategyMentionState(el.value, el.selectionStart ?? el.value.length);
    }
    const mentionOpen = !!(ui as any).strategyMentionOpen;
    const mentionSuggestions = Array.isArray((ui as any).strategyMentionSuggestions) ? ((ui as any).strategyMentionSuggestions as StrategyMentionSuggestion[]) : [];
    if (mentionOpen && mentionSuggestions.length) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const dir = e.key === 'ArrowDown' ? 1 : -1;
        const currentIndex = Number((ui as any).strategyMentionSelectedIndex || 0);
        const nextIndex = (currentIndex + dir + mentionSuggestions.length) % mentionSuggestions.length;
        (ui as any).strategyMentionSelectedIndex = nextIndex;
        render();
        focusStrategyChatComposer();
        return;
      }
      if ((e.key === 'Enter' && !e.shiftKey) || e.key === 'Tab') {
        e.preventDefault();
        const selectedIndex = Math.max(0, Math.min(Number((ui as any).strategyMentionSelectedIndex || 0), mentionSuggestions.length - 1));
        applyStrategyMentionSuggestion(mentionSuggestions[selectedIndex]);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        closeStrategyMentionMenu();
        render();
        focusStrategyChatComposer();
        return;
      }
    }
    if (e.key === '#') {
      e.preventDefault();
      openStrategyActionPrompt(el.selectionStart ?? el.value.length);
      return;
    }
    if (e.key !== 'Enter' || e.shiftKey) return;
    if ('isComposing' in e && (e as KeyboardEvent).isComposing) return;
    e.preventDefault();
    void sendStrategyAgentMessage();
  });

  document.addEventListener('keydown', (e) => {
    const el = (e.target as HTMLElement | null)?.closest<HTMLInputElement>('[data-action="draft-strategy-action"]');
    if (!el) return;
    if (e.key === 'Enter') {
      e.preventDefault();
      applyStrategyActionPrompt();
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      closeStrategyActionPrompt();
      render();
      focusStrategyChatComposer();
    }
  });

  document.addEventListener('input', (e) => {
    const el = (e.target as HTMLElement | null)?.closest<HTMLTextAreaElement>('[data-action="draft-strategy-chat"][data-field="input"]');
    if (!el) return;
    autoResizeStrategyChatArea(el);
    queueStrategyChatDockedComposerSync();
  });

  document.addEventListener('scroll', (e) => {
    const el = (e.target as HTMLElement | null)?.closest?.('[data-action="draft-strategy-chat"][data-field="input"]') as HTMLTextAreaElement | null;
    if (!el) return;
    syncStrategyChatComposerMirror(el);
  }, true);

  document.addEventListener('focusin', (e) => {
    const el = (e.target as HTMLElement | null)?.closest?.('[data-action="draft-strategy-chat"][data-field="input"]') as HTMLTextAreaElement | null;
    if (!el) return;
    syncStrategyChatComposerMirror(el);
  });

  document.addEventListener('click', (e) => {
    const target = e.target as HTMLElement | null;
    if (target?.closest?.('.strategyMentionPanel') || target?.closest?.('[data-action="draft-strategy-chat"][data-field="input"]')) {
      const field = target.closest?.('[data-action="draft-strategy-chat"][data-field="input"]') as HTMLTextAreaElement | null;
      if (field) {
        syncStrategyMentionState(field.value, field.selectionStart ?? field.value.length);
      }
      return;
    }
    if ((ui as any).strategyMentionOpen) {
      closeStrategyMentionMenu();
      render();
    }
  });

  document.addEventListener('input', (e) => {
    const el = (e.target as HTMLElement | null)?.closest<HTMLTextAreaElement>('[data-action="draft-strategy-record-edit"][data-field="content"]');
    if (!el) return;
    autoResizeStrategyChatArea(el);
  });

  document.addEventListener('input', (e) => {
    const el = (e.target as HTMLElement | null)?.closest<HTMLInputElement>('[data-action="draft-relationship-group-title"],[data-action="draft-relationship-member-name"]');
    if (!el) return;
    const draft = ensureClassRosterDraft() as any[];
    const groupIndex = Number(el.dataset.groupIndex ?? '-1');
    if (Number.isNaN(groupIndex) || groupIndex < 0 || groupIndex >= draft.length) return;
    if (el.dataset.action === 'draft-relationship-group-title') {
      draft[groupIndex].title = String(el.value || '');
    } else {
      const memberIndex = Number(el.dataset.memberIndex ?? '-1');
      if (Number.isNaN(memberIndex) || memberIndex < 0) return;
      draft[groupIndex].members = Array.isArray(draft[groupIndex].members) ? draft[groupIndex].members : [];
      if (!draft[groupIndex].members[memberIndex]) {
        draft[groupIndex].members[memberIndex] = { id: `${draft[groupIndex].id}-member-${memberIndex + 1}`, name: '' };
      }
      draft[groupIndex].members[memberIndex].name = String(el.value || '');
    }
    updateClassRosterCountUI();
  });

  document.addEventListener('cancel', (e) => {
    const t = e.target as HTMLElement | null; if (!t) return;
    if ((t as any).id === SCREEN_PIN_MODAL_ID && ui.pinLocked) {
      e.preventDefault();
    }
  }, true);

  document.addEventListener('close', (e) => {
    const t = e.target as HTMLElement | null; if (!t) return;
    if (_isRerendering) return;
    if ((t as any).id === 'recordModal') { ui.viewRecordId = null; ui.recordEditId = null; ui.recordModalTab = 'current'; ui.recEditRelatedOpen = false; resetRecordEditDraft(); }
    if ((t as any).id === 'recordComposerModal') ui.recordComposerOpen = false;
    if ((t as any).id === 'paperPickModal') ui.paperPickOpen = false;
    if ((t as any).id === 'paperModal') (ui.paperCaseId = null, ui.paperHash = null);
    if ((t as any).id === 'caseUpdateModal') (ui.updateCaseId = null, ui.updatePickIds = [], ui.updFilterActor = ui.updFilterPlace = ui.updFilterKeyword = '', ui.updFilterActorDraft = ui.updFilterPlaceDraft = ui.updFilterKeywordDraft = '', ui.updateCandidatesForCaseId = null, ui.updateCandidates = null, ui.updateCandidatesLoading = false);
    if ((t as any).id === 'settingsModal') { ui.settingsOpen = false; resetScreenPinSettingsDraft(); }
    if ((t as any).id === 'updateNotesModal') ui.updatesNoteOpen = false;
    if ((t as any).id === 'simulationPickerModal') { (ui as any).simulationPickerOpen = false; (ui as any).simulationPickerQuery = ''; (ui as any).simulationPickerSelectedRecordIds = []; }
    if ((t as any).id === 'classRosterModal') { ui.classRosterOpen = false; ui.classRosterDraft = cloneRelationshipGroups(getRelationshipGroups()); ui.classRosterGroupId = String(ui.classRosterDraft[0]?.id || 'group-1'); }
    if ((t as any).id === SIGNATURE_MODAL_ID) ui.signatureModalMode = null;
    if ((t as any).id === SCREEN_PIN_MODAL_ID) {
      if (ui.pinLocked) {
        ui.pinModalOpen = true;
        window.setTimeout(() => openDlg(SCREEN_PIN_MODAL_ID), 0);
        return;
      }
      ui.pinModalOpen = false;
      resetScreenPinModalDraft();
    }
  }, true);

  window.addEventListener('keydown', (e) => {
    const ae0 = document.activeElement as HTMLElement | null;
    if (ae0 && (ae0 as any).id === 'restoreDropZone' && (e.key === 'Enter' || e.key === ' ')) {
      e.preventDefault();
      const input = document.getElementById('restoreFile') as HTMLInputElement | null;
      input?.click();
      return;
    }


    // Enter로 필터 적용(메모 필터 / 업데이트 필터)
    if (e.key === 'Enter' && !e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey) {
      const ae = document.activeElement as HTMLElement | null;
      if (ae?.closest('[data-action="draft-record-filters"]')) {
        e.preventDefault();
        (document.querySelector('[data-action="apply-record-filters"]') as HTMLButtonElement | null)?.click();
        return;
      }
      if (ae?.closest('[data-action="draft-update-filters"]')) {
        e.preventDefault();
        (document.querySelector('[data-action="apply-update-filters"]') as HTMLButtonElement | null)?.click();
        return;
      }
    }

    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      const ae = document.activeElement as HTMLElement | null; if (!ae) return;
      if (ae.closest('[data-action="draft-record"][data-field^="summary"]')) return void (e.preventDefault(), (document.querySelector('[data-action="save-record"]') as HTMLButtonElement | null)?.click());
      if (ae.closest('[data-action="draft-record-edit"][data-field^="summary"]')) return void (e.preventDefault(), (document.querySelector('[data-action="save-record-amend"]') as HTMLButtonElement | null)?.click());
      if (ae.closest('[data-action="draft-step"][data-field="note"]')) return void (e.preventDefault(), (document.querySelector('[data-action="add-step"]') as HTMLButtonElement | null)?.click());
    }

    if (e.key === 'Escape') {
      const c = dlg('confirmModal'); if (isDialogEl(c) && c.open) return void (e.preventDefault(), closeConfirm(false));
      const sg = dlg(SIGNATURE_MODAL_ID); if (isDialogEl(sg) && sg.open) return void (e.preventDefault(), closeSignatureModal());
      const ss = dlg(SIGN_SUCCESS_MODAL_ID); if (isDialogEl(ss) && ss.open) return void (e.preventDefault(), closeDlg(SIGN_SUCCESS_MODAL_ID));
      const sm = dlg('savedModal'); if (isDialogEl(sm) && sm.open) return void (e.preventDefault(), closeDlg('savedModal'));
      const cm = dlg('caseCreatedModal'); if (isDialogEl(cm) && cm.open) return void (e.preventDefault(), closeDlg('caseCreatedModal'));
      const pin = dlg(SCREEN_PIN_MODAL_ID); if (isDialogEl(pin) && pin.open && ui.pinLocked) return void (e.preventDefault());
      if (isDialogEl(pin) && pin.open) return void (e.preventDefault(), ui.pinModalOpen = false, resetScreenPinModalDraft(), closeDlg(SCREEN_PIN_MODAL_ID), render());
      closeDlg('restoreModal'); closeDlg('logsModal');
      const st = dlg('settingsModal'); if (isDialogEl(st) && st.open) return void (e.preventDefault(), ui.settingsOpen = false, closeDlg('settingsModal'));
      const un = dlg('updateNotesModal'); if (isDialogEl(un) && un.open) return void (e.preventDefault(), ui.updatesNoteOpen = false, closeDlg('updateNotesModal'), render());
      const picker = dlg('simulationPickerModal'); if (isDialogEl(picker) && picker.open) return void (e.preventDefault(), (ui as any).simulationPickerOpen = false, (ui as any).simulationPickerQuery = '', (ui as any).simulationPickerSelectedRecordIds = [], closeDlg('simulationPickerModal'), render());
      const roster = dlg('classRosterModal'); if (ui.classRosterOpen || (isDialogEl(roster) && roster.open)) return void (e.preventDefault(), ui.classRosterOpen = false, ui.classRosterDraft = cloneRelationshipGroups(getRelationshipGroups()), ui.classRosterGroupId = String(ui.classRosterDraft[0]?.id || 'group-1'), closeDlg('classRosterModal'), render());
      const composer = dlg('recordComposerModal'); if (isDialogEl(composer) && composer.open) return void (e.preventDefault(), ui.recordComposerOpen = false, closeDlg('recordComposerModal'), render());
      const rec = dlg('recordModal'); if (isDialogEl(rec) && rec.open) return void (e.preventDefault(), closeRecordModal(), render());
      const tl = dlg('timelineDetailModal'); if (isDialogEl(tl) && tl.open) return void (e.preventDefault(), closeTimelineModal(), render());
      const cu = dlg('caseUpdateModal'); if (isDialogEl(cu) && cu.open) return void (e.preventDefault(), closeCaseUpdateModal(), render());
    }
  });
}

function syncDraftDefaults() {
  ui.classRosterDraft = cloneRelationshipGroups(getRelationshipGroups());
  ui.classRosterGroupId = String(ui.classRosterDraft[0]?.id || 'group-1');
  ui.pinLocked = false;
  ui.pinModalOpen = false;
  resetScreenPinModalDraft();
  resetScreenPinSettingsDraft();
  draftRecord.actorNameChoice = OTHER; draftRecord.relNameChoice = OTHER; draftRecordEdit.actorNameChoice = OTHER; draftRecordEdit.relNameChoice = OTHER;
  (draftRecord as any).placeText ||= draftRecord.place; (draftRecord as any).storeTypeText ||= draftRecord.storeType; (draftRecord as any).lvText ||= draftRecord.lv;
  (draftRecord as any).actorTypeText ||= actorTypeTextFromInternal(draftRecord.actorType); (draftRecord as any).relTypeText ||= actorTypeTextFromInternal(draftRecord.relType);
  (draftRecord as any).actorGroupId ||= String(getRelationshipGroups()[0]?.id || 'group-1');
  (draftRecord as any).relGroupId ||= String(getRelationshipGroups()[0]?.id || 'group-1');
  (draftRecord as any).signerLabel ||= '기기 봉인서명'; (draftRecord as any).sealReason ||= '';
  (draftRecordEdit as any).placeText ||= draftRecordEdit.place; (draftRecordEdit as any).storeTypeText ||= draftRecordEdit.storeType; (draftRecordEdit as any).lvText ||= draftRecordEdit.lv;
  (draftRecordEdit as any).actorTypeText ||= actorTypeTextFromInternal(draftRecordEdit.actorType); (draftRecordEdit as any).relTypeText ||= actorTypeTextFromInternal(draftRecordEdit.relType);
  (draftRecordEdit as any).signerLabel ||= '기기 봉인서명'; (draftRecordEdit as any).sealReason ||= '';
  (draftCase as any).sensFilterText ||= String(draftCase.sensFilter); (draftCase as any).statusText ||= draftCase.status;
  syncClassRosterFieldDefaults();
}

export function initApp() {
  bindEvents(); ensurePaperStyles(); syncDraftDefaults();
  ensureStrategyChatProgressListener();

  const focusRecordComposer = () => window.setTimeout(() => {
    (document.getElementById('recordSummaryOverview') as HTMLTextAreaElement | null)?.focus();
  }, 0);

  // ✅ 앱 실행 시 첫 화면: 홈
  (S as any).tab = 'home' as any; S.tab = 'home' as any; render();
  window.setTimeout(() => {
    maybeStartStrategyBackendPrewarm('app_init');
  }, 160);

  (async () => {
    try {
      await refreshDeviceSignerInfo();
      setState(await reverifyStateRecords(await loadState()));
      log('state loaded');
    }
    catch (e) { log('load failed', e); setState(defaultState()); }

    // ✅ 로컬에 마지막 탭이 무엇이었든, 실행 시작은 home으로 고정
    (S as any).tab = 'home' as any; S.tab = 'home' as any;

    syncDraftDefaults(); render();
  })();
}
queueMicrotask(() => {
  if (!hasTauriWindow() || !isWindowsDesktop()) return;
  void refreshStrategyModelStatus().catch((err) => {
    log('initial strategy model status failed', err);
  });
});
