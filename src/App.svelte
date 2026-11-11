<script lang="ts">
  import { isTauri } from '@tauri-apps/api/core';
  import { emit, listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount } from 'svelte';
  import type { CommandResponse } from '../crates/orbit-core/bindings/CommandResponse';
  import type { DashInfo } from '../crates/orbit-core/bindings/DashInfo';
  import type { DockPosition } from '../crates/orbit-core/bindings/DockPosition';
  import type { OcrResult } from '../crates/orbit-core/bindings/OcrResult';
  import type { OverlaySnapshot } from '../crates/orbit-core/bindings/OverlaySnapshot';
  import type { RecordStateEvent } from '../crates/orbit-core/bindings/RecordStateEvent';
  import type { RecoveryItem } from '../crates/orbit-core/bindings/RecoveryItem';
  import type { RegionSelectionStart } from '../crates/orbit-core/bindings/RegionSelectionStart';
  import type { SetupResult } from '../crates/orbit-core/bindings/SetupResult';
  import type { WindowLayout } from '../crates/orbit-core/bindings/WindowLayout';
  import type { WindowSize } from '../crates/orbit-core/bindings/WindowSize';
  import { runCommand } from './bus';
  import { open } from '@tauri-apps/plugin-dialog';
  import ClipboardPanel from './ClipboardPanel.svelte';
  import DiscSettings from './DiscSettings.svelte';
  import SystemMonitor from './SystemMonitor.svelte';
  import OrbGalaxy from './OrbGalaxy.svelte';

  // 选区矩形是前端本地交互状态；其余类型全部来自 ts-rs 生成的契约绑定。
  type RegionBounds = { left: number; top: number; width: number; height: number };

  const desktopBridgeAvailable = isTauri();
  // 同一份前端按窗口标签分流：clipboard 窗口只渲染剪贴板面板
  const isClipboardWindow = desktopBridgeAvailable && getCurrentWindow().label === 'clipboard';
  let dockPosition: DockPosition = 'floating';
  let panelOpen = false;
  let setupRunning = false;
  let captureRunning = false;
  let autostartConfigured = false;
  let setupAttempted = false;
  let permissionStates: SetupResult['permissions'] = {};
  let notice = '';
  let dragStart: { pointerId: number; x: number; y: number } | null = null;
  let suppressClick = false;
  let selectingRegion = false;
  let selectionMonitorId: number | null = null;
  let selectionStart: { x: number; y: number } | null = null;
  let selectionBounds: RegionBounds | null = null;
  let orbHovered = false;
  let galaxyRef: OrbGalaxy | null = null;
  let flashIndex = -1;
  let convertInput: string | null = null;
  let convertBusy = false;
  const CONVERT_FORMATS = [
    { id: 'mp3', label: 'MP3 音频' },
    { id: 'wav', label: 'WAV 音频' },
    { id: 'mp4', label: 'MP4 视频' },
    { id: 'gif', label: 'GIF 动图' },
    { id: 'webp', label: 'WebP 动图' },
  ];

  // 光标视差：把球内光标相对偏移传给 galaxy
  function handleOrbParallax(event: MouseEvent): void {
    if (!galaxyRef) return;
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const px = (event.clientX - rect.left) / rect.width - 0.5;
    const py = (event.clientY - rect.top) / rect.height - 0.5;
    galaxyRef.setParallax(px, py);
  }

  // ============ 圆盘几何 ============
  // 展开窗口尺寸由 overlay.layout 下发；viewBox 固定 340×340 作为几何基准坐标系，
  // SVG 容器随窗口 100% 缩放，配置尺寸变化时圆盘自动等比映射，无需改写半径常量。
  // 悬浮球半径 = 41px（orb-button 宽 82px 的一半）
  // 内圆 / 外圆半径可在编辑页拖动，范围受窗口与最小扇区宽度约束
  const VIEW = 340;
  const DCX = VIEW / 2, DCY = VIEW / 2;
  const ORB_RADIUS = 41;
  const R_IN_MIN = ORB_RADIUS + 3;  // 44
  const R_IN_MAX = 80;
  const R_OUT_MIN = 96;
  const R_OUT_MAX = VIEW / 2 - 38;  // 132 — 给顶部提示与底部按钮留出空位
  const MIN_RING_WIDTH = 26;
  const DEFAULT_DISC_GEOMETRY = { rIn: ORB_RADIUS + 6, rOut: 112 };
  const DISC_GEOMETRY_KEY = 'orbit.discGeometry';
  const GAP = 2.0 * Math.PI / 180;

  function pt(r: number, a: number): [number, number] {
    return [DCX + Math.cos(a) * r, DCY + Math.sin(a) * r];
  }
  function sectorPath(rIn: number, rOut: number, a0: number, a1: number): string {
    const [x1, y1] = pt(rOut, a0);
    const [x2, y2] = pt(rOut, a1);
    const [x3, y3] = pt(rIn,  a1);
    const [x4, y4] = pt(rIn,  a0);
    const large = (a1 - a0) > Math.PI ? 1 : 0;
    return `M ${x1} ${y1} A ${rOut} ${rOut} 0 ${large} 1 ${x2} ${y2} `
         + `L ${x3} ${y3} A ${rIn} ${rIn} 0 ${large} 0 ${x4} ${y4} Z`;
  }

  // ============ 圆盘功能项 ============
  interface DiscAction {
    id: string;
    icon: string;
    label: string;
    handler: () => void | Promise<void>;
  }
  const actions: DiscAction[] = [
    { id: 'screenshot.full', icon: '📷', label: '全屏截图', handler: captureScreenshot },
    { id: 'screenshot.region', icon: '✂️', label: '区域截图', handler: beginRegionScreenshot },
    { id: 'record', icon: '⏺', label: '录屏', handler: toggleRecord },
    { id: 'clipboard', icon: '📋', label: '剪贴板', handler: showClipHistory },
    { id: 'ocr', icon: '🔍', label: 'OCR', handler: runOcr },
    { id: 'input', icon: '🎮', label: '键鼠录制', handler: toggleKbRecord },
    { id: 'format', icon: '🔄', label: '格式转换', handler: runConvert },
    { id: 'mcp', icon: '🔌', label: 'MCP 工具', handler: showMcp },
    { id: 'theme', icon: '🎨', label: '主题', handler: cycleTheme },
    { id: 'monitor', icon: '📊', label: '系统监控', handler: showSystemMonitor },
    { id: 'setup', icon: '⚙️', label: '设置', handler: openSettings },
    { id: 'close', icon: '✕', label: '关闭', handler: () => { void setPanelOpen(false); } },
  ];

  const ACTION_MAP = new Map(actions.map((action) => [action.id, action]));
  const DEFAULT_DISC_LAYOUT = actions.map((action) => action.id);
  const DISC_LAYOUT_KEY = 'orbit.discLayout';

  function sanitizeDiscLayout(value: unknown): string[] | null {
    if (!Array.isArray(value)) return null;
    const seen = new Set<string>();
    const layout: string[] = [];
    for (const id of value) {
      if (typeof id !== 'string' || seen.has(id) || !ACTION_MAP.has(id)) continue;
      seen.add(id);
      layout.push(id);
    }
    return layout.length > 0 ? layout : null;
  }

  function loadDiscLayout(): string[] {
    try {
      const raw = localStorage.getItem(DISC_LAYOUT_KEY);
      if (!raw) return [...DEFAULT_DISC_LAYOUT];
      return sanitizeDiscLayout(JSON.parse(raw)) ?? [...DEFAULT_DISC_LAYOUT];
    } catch {
      return [...DEFAULT_DISC_LAYOUT];
    }
  }

  function sanitizeDiscGeometry(value: unknown): { rIn: number; rOut: number } | null {
    if (!value || typeof value !== 'object') return null;
    const { rIn, rOut } = value as Record<string, unknown>;
    if (typeof rIn !== 'number' || typeof rOut !== 'number') return null;
    if (!Number.isFinite(rIn) || !Number.isFinite(rOut)) return null;
    const safeIn = Math.min(R_IN_MAX, Math.max(R_IN_MIN, Math.round(rIn)));
    const safeOut = Math.min(R_OUT_MAX, Math.max(R_OUT_MIN, Math.round(rOut)));
    if (safeOut - safeIn < MIN_RING_WIDTH) return null;
    return { rIn: safeIn, rOut: safeOut };
  }

  function loadDiscGeometry(): { rIn: number; rOut: number } {
    try {
      const raw = localStorage.getItem(DISC_GEOMETRY_KEY);
      if (!raw) return { ...DEFAULT_DISC_GEOMETRY };
      return sanitizeDiscGeometry(JSON.parse(raw)) ?? { ...DEFAULT_DISC_GEOMETRY };
    } catch {
      return { ...DEFAULT_DISC_GEOMETRY };
    }
  }

  let discLayout: string[] = loadDiscLayout();
  let discGeometry = loadDiscGeometry();
  let draftGeometry = { ...discGeometry };
  let settingsOpen = false;
  let editMode = false;
  let draftLayout: string[] = [];
  let trayOpen = false;
  let discSvg: SVGSVGElement | null = null;
  let editDrag: { pointerId: number; index: number; startX: number; startY: number; active: boolean } | null = null;
  let resizeDrag: { pointerId: number; kind: 'inner' | 'outer' } | null = null;
  let editTarget = -1;
  let editRemove = false;
  $: viewRIn = editMode ? draftGeometry.rIn : discGeometry.rIn;
  $: viewROut = editMode ? draftGeometry.rOut : discGeometry.rOut;

  $: discActions = discLayout
    .map((id) => ACTION_MAP.get(id))
    .filter((action): action is DiscAction => Boolean(action));
  $: draftActions = draftLayout
    .map((id) => ACTION_MAP.get(id))
    .filter((action): action is DiscAction => Boolean(action));
  $: viewActions = editMode ? draftActions : discActions;
  $: sectorCount = viewActions.length || 1;
  $: unusedActions = actions.filter((action) => !draftLayout.includes(action.id));

  function segClick(idx: number) {
    const action = discActions[idx];
    // 清掉悬浮球拖拽残留，避免拖动逻辑吞掉这次点击
    dragStart = null;
    suppressClick = false;
    flashIndex = idx;
    notice = `⏳ ${action.label}…`;
    setTimeout(() => { flashIndex = -1; }, 220);
    // 立即执行，不再延迟；失败也能给出可见反馈
    void Promise.resolve(action.handler()).catch((error: unknown) => {
      notice = `${action.label}失败：${String(error)}`;
    });
  }

  // ============ 灵动岛仪表盘 ============
  let dashInfo: DashInfo | null = null;

  function formatClock() {
    const d = new Date();
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}:${String(d.getSeconds()).padStart(2, '0')}`;
  }

  let clockNow = formatClock();
  $: if (dockPosition === 'top' || dockPosition === 'floating') {
    // 每秒刷新时钟
  }

  // ============ 主题系统 ============
  const THEMES = ['orbit-white', 'cosmic-violet', 'pure-black', 'cyber-blue', 'mono-light'] as const;
  const THEME_LABELS: Record<(typeof THEMES)[number], string> = {
    'orbit-white': 'Orbit 白',
    'cosmic-violet': '深空紫',
    'pure-black': '纯黑极简',
    'cyber-blue': '赛博蓝',
    'mono-light': '极简白',
  };
  let currentTheme: (typeof THEMES)[number] = 'orbit-white';

  function applyTheme(theme: (typeof THEMES)[number]) {
    currentTheme = theme;
    document.documentElement.dataset.theme = theme;
    try { localStorage.setItem('orbit.theme', theme); } catch { /* ignore */ }
    notice = `主题已切换：${THEME_LABELS[theme]}`;
  }
  function cycleTheme() {
    const idx = THEMES.indexOf(currentTheme);
    applyTheme(THEMES[(idx + 1) % THEMES.length]);
  }

  // ============ 录屏控制 ============
  // 录制期间悬浮球显示计时与控制按钮，暂停/停止由前端按钮触发；
  // 是否录制、是否暂停只能靠 record-state 事件回流保持一致。
  const RECORD_DIR_KEY = 'orbit.record.dir';
  let recordDir: string | null = localStorage.getItem(RECORD_DIR_KEY);
  let recordingActive = false;
  let recordingPaused = false;
  let recordElapsedMs = 0;
  let recordTimer: number | null = null;

  function formatRecTime(ms: number): string {
    const totalSec = Math.floor(ms / 1000);
    const m = Math.floor(totalSec / 60);
    const s = totalSec % 60;
    return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
  }

  function startRecTimer(): void {
    if (recordTimer !== null) return;
    recordTimer = window.setInterval(() => {
      if (!recordingPaused) recordElapsedMs += 250;
    }, 250);
  }

  function stopRecTimer(): void {
    if (recordTimer !== null) {
      clearInterval(recordTimer);
      recordTimer = null;
    }
  }

  function handleRecordState(state: RecordStateEvent): void {
    switch (state.phase) {
      case 'started':
        recordingActive = true;
        recordingPaused = state.paused;
        if (recordingPaused) {
          notice = '⏸ 录制已暂停；点击 ▶ 继续';
        } else {
          notice = '⏺ 录屏中；点击 ⏸ 暂停，⏹ 停止并保存';
          startRecTimer();
        }
        break;
      case 'saving':
        recordingPaused = false;
        stopRecTimer();
        notice = '⏺ 正在结束录屏并保存…';
        break;
      case 'saved':
        recordingActive = false;
        recordingPaused = false;
        stopRecTimer();
        recordElapsedMs = 0;
        notice = state.path ? `✅ 录屏已保存：${state.path}` : '✅ 录屏已停止';
        break;
      case 'recovering':
        recordingActive = false;
        recordingPaused = false;
        stopRecTimer();
        recordElapsedMs = 0;
        void runCommand<RecoveryItem[]>('record.recovery.list')
          .then((items) => {
            notice = `⚠️ ${state.message ?? '录屏收尾异常'}；恢复区现有 ${items.length} 个文件${state.path ? `：${state.path}` : ''}`;
          })
          .catch(() => {
            notice = `⚠️ ${state.message ?? '录屏收尾异常'}${state.path ? `：${state.path}` : ''}`;
          });
        break;
      default:
        break;
    }
  }

  // 未配置保存目录时弹窗选择；配置过则直接复用（设置面板可随时修改）。
  async function ensureRecordDir(): Promise<string | null> {
    const existing = localStorage.getItem(RECORD_DIR_KEY);
    if (existing) {
      recordDir = existing;
      return existing;
    }
    if (!desktopBridgeAvailable) { notice = '请在桌面端使用录屏。'; return null; }
    try {
      const picked = await open({ directory: true, multiple: false, title: '选择录屏保存位置' });
      if (!picked || typeof picked !== 'string') return null;
      localStorage.setItem(RECORD_DIR_KEY, picked);
      recordDir = picked;
      return picked;
    } catch (e) { notice = `选择目录失败：${String(e)}`; return null; }
  }

  async function toggleRecord(): Promise<void> {
    if (!desktopBridgeAvailable) { notice = '请在桌面端使用录屏。'; return; }
    if (!recordingActive) {
      const dir = await ensureRecordDir();
      if (!dir) { notice = '未选择保存位置，已取消录制。'; return; }
      try {
        await runCommand<string>('record.start', { outputDir: dir });
        recordingActive = true;
        recordingPaused = false;
        recordElapsedMs = 0;
        startRecTimer();
      } catch (e) { notice = `录屏启动失败：${String(e)}`; }
    } else {
      await togglePause();
    }
  }

  async function togglePause(): Promise<void> {
    if (!recordingActive) return;
    try {
      if (!recordingPaused) await runCommand('record.pause');
      else await runCommand('record.resume');
    } catch (e) { notice = `暂停/继续失败：${String(e)}`; }
  }

  async function stopRecord(): Promise<void> {
    try {
      await runCommand<string | null>('record.stop');
    } catch (e) { notice = `录屏停止失败：${String(e)}`; }
  }

  // 设置面板里主动选择/清除录屏保存目录（与 ensureRecordDir 共用 localStorage）。
  async function pickRecordDir(): Promise<void> {
    if (!desktopBridgeAvailable) return;
    try {
      const picked = await open({ directory: true, multiple: false, title: '选择录屏保存位置' });
      if (picked && typeof picked === 'string') {
        localStorage.setItem(RECORD_DIR_KEY, picked);
        recordDir = picked;
      }
    } catch (e) { notice = `选择目录失败：${String(e)}`; }
  }

  function clearRecordDir(): void {
    localStorage.removeItem(RECORD_DIR_KEY);
    recordDir = null;
  }

  async function runOcr() {
    if (!desktopBridgeAvailable) { notice = '请在桌面端使用 OCR。'; return; }
    notice = '🔍 正在截屏+识别…';
    try {
      const result = await runCommand<OcrResult>('ocr.current_screen');
      await runCommand('clipboard.write', { text: result.text });
      const words = result.lines.reduce((total, line) => total + line.words.length, 0);
      notice = `✅ OCR 完成（${result.text.length}字 / ${result.lines.length}行 / ${words}词，带坐标），已写回剪贴板：${result.text.slice(0, 30)}…`;
    } catch (e) { notice = `OCR 失败：${String(e)}`; }
  }

  let kbRecording = false;
  async function toggleKbRecord() {
    if (!desktopBridgeAvailable) { notice = '请在桌面端使用。'; return; }
    if (!kbRecording) {
      try { await runCommand('kb_record.start'); kbRecording = true; notice = '🎮 键鼠录制中…再次点击扇区停止并保存'; }
      catch (e) { notice = `录制启动失败：${String(e)}`; }
    } else {
      try { await runCommand('kb_record.stop'); kbRecording = false; notice = '🎮 录制已停止并保存到本地'; }
      catch (e) { notice = `停止失败：${String(e)}`; }
    }
  }

  async function runConvert() {
    if (!desktopBridgeAvailable) { notice = '请在桌面端使用。'; return; }
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const input = await open({ title: '选择要转换的文件', multiple: false });
      if (!input || typeof input !== 'string') { notice = '已取消'; return; }
      convertInput = input;
      notice = '🔄 选择目标格式';
    } catch (e) { notice = `选择文件失败：${String(e)}`; }
  }

  async function chooseConvertFormat(format: string) {
    if (!convertInput || convertBusy) return;
    const input = convertInput;
    try {
      const { save } = await import('@tauri-apps/plugin-dialog');
      const outPath = await save({
        title: `保存为 .${format}`,
        defaultPath: input.replace(/\.[^.]+$/, `.${format}`),
      });
      if (!outPath) return;
      convertBusy = true;
      notice = `🔄 正在转换为 ${format.toUpperCase()}…`;
      const result = await runCommand<string>('convert.format', {
        inputPath: input,
        outputPath: outPath,
        format,
      });
      notice = `✅ 转换完成：${result}`;
      convertInput = null;
    } catch (e) {
      notice = `转换失败：${String(e)}`;
    } finally {
      convertBusy = false;
    }
  }

  function cancelConvert() {
    convertInput = null;
    notice = '已取消转换';
  }

  function showMcp() {
    notice = '🔌 MCP sidecar 待接入（需要打包 mcp server 二进制）';
  }
  async function showSystemMonitor() {
    if (!desktopBridgeAvailable) { notice = '请在 Orbit 桌面端使用系统监控。'; return; }
    await setPanelOpen(true);
    monitorOpen = true;
    settingsOpen = false;
    convertInput = null;
    trayOpen = false;
  }
  let monitorOpen = false;
  // ============ 剪贴板：采集/存储已下沉到 Rust + SQLite，前端只负责唤起面板 ============
  let dashPollTimer: ReturnType<typeof setInterval> | null = null;
  let clockTimer: ReturnType<typeof setInterval> | null = null;

  async function pollDashInfo() {
    if (!desktopBridgeAvailable) return;
    try {
      dashInfo = await runCommand<DashInfo>('system.dash');
    } catch { /* 保留上一次真实采样，不用占位值覆盖 */ }
  }

  async function showClipHistory(): Promise<void> {
    if (!desktopBridgeAvailable) { notice = '请在 Orbit 桌面端使用剪贴板历史。'; return; }
    try {
      await runCommand('clipboard.open_window');
      await setPanelOpen(false);
    } catch (error) {
      notice = `打开剪贴板失败：${String(error)}`;
    }
  }

  // 窗口尺寸的唯一前端入口：把配置注入 :root，供所有弹窗与圆盘使用
  function applyWindowLayout(layout: WindowLayout): void {
    const style = document.documentElement.style;
    const setSize = (name: string, size: WindowSize): void => {
      style.setProperty(`--orb-${name}-w`, `${size.width}px`);
      style.setProperty(`--orb-${name}-h`, `${size.height}px`);
    };
    setSize('floating', layout.floating);
    setSize('side', layout.side);
    setSize('notch', layout.notch);
    setSize('expanded', layout.expanded);
    setSize('clipboard', layout.clipboard);
    setSize('selection', layout.selection);
    style.setProperty('--orb-panel-inset', `${layout.panelInset}px`);
  }

  // 初始化：从 localStorage 读主题
  onMount(() => {
    try {
      const saved = localStorage.getItem('orbit.theme') as (typeof THEMES)[number] | null;
      if (saved && THEMES.includes(saved)) applyTheme(saved);
    } catch { /* ignore */ }

    // 三个窗口（悬浮球 / 剪贴板 / 选区）都读取同一份尺寸配置
    if (desktopBridgeAvailable) {
      void runCommand<WindowLayout>('overlay.layout')
        .then(applyWindowLayout)
        .catch((error: unknown) => {
          console.warn('窗口尺寸配置读取失败，使用 CSS 兜底值', error);
        });
    }

    // 剪贴板窗口由 ClipboardPanel 自行接管，不注册悬浮层事件与仪表盘轮询
    if (isClipboardWindow) return;

    pollDashInfo();
    dashPollTimer = setInterval(pollDashInfo, 2000);
    clockNow = formatClock();
    clockTimer = setInterval(() => { clockNow = formatClock(); }, 1000);
    if (!desktopBridgeAvailable) return;

    const currentWindow = getCurrentWindow();
    if (currentWindow.label === 'selection') {
      let disposed = false;
      let unlisten: (() => void) | undefined;
      void listen<RegionSelectionStart>('region-selection-start', ({ payload }) => {
        selectionMonitorId = payload.monitorId;
        selectionStart = null;
        selectionBounds = null;
        selectingRegion = true;
      })
        .then((dispose) => {
          if (disposed) dispose();
          else unlisten = dispose;
        })
        .catch((error: unknown) => {
          void runCommand('region.cancel').then(
            () => emit('region-capture-result', { error: `区域截图界面初始化失败：${String(error)}` }),
            (cancelError: unknown) =>
              emit('region-capture-result', {
                error: `区域截图界面初始化失败：${String(error)}；恢复窗口失败：${String(cancelError)}`,
              }),
          );
        });
      return () => {
        disposed = true;
        unlisten?.();
      };
    }

    let disposed = false;
    let moveTimer: ReturnType<typeof setTimeout> | undefined;
    let unlisteners: (() => void)[] = [];

    void Promise.all([
      listen<{ response?: CommandResponse; error?: string; cancelled?: boolean }>(
        'region-capture-result',
        ({ payload }) => {
          captureRunning = false;
          notice = payload.cancelled
            ? '已取消区域截图。'
            : payload.error
            ? `区域截图失败：${payload.error}`
            : payload.response?.status === 'ok'
              ? '区域截图已复制到剪贴板。'
              : `区域截图失败：${payload.response?.error ?? '截图命令执行失败。'}`;
        },
      ),
      listen<RecordStateEvent>('record-state', ({ payload }) => {
        handleRecordState(payload);
      }),
      getCurrentWindow().onMoved(() => {
        if (moveTimer) clearTimeout(moveTimer);
        moveTimer = setTimeout(() => {
          void runCommand<OverlaySnapshot>('overlay.dock')
            .then((layout) => {
              dockPosition = layout.dock;
              panelOpen = layout.expanded;
            })
            .catch((error: unknown) => {
              notice = `贴边吸附失败：${String(error)}`;
            });
        }, 220);
      }),
    ])
      .then((disposeListeners) => {
        if (disposed) {
          disposeListeners.forEach((dispose) => dispose());
          return;
        }
        unlisteners = disposeListeners;
        void runCommand<SetupResult>('app.setup.status')
          .then((result) => {
            autostartConfigured = result.autostartConfigured;
            setupAttempted = result.autostartConfigured;
            permissionStates = result.permissions;
          })
          .catch((error: unknown) => {
            notice = `读取设置状态失败：${String(error)}`;
          });
        void runCommand<OverlaySnapshot>('overlay.dock')
          .then((layout) => {
            dockPosition = layout.dock;
            panelOpen = layout.expanded;
          })
          .catch((error: unknown) => {
            notice = `悬浮球位置初始化失败：${String(error)}`;
          });
      })
      .catch((error: unknown) => {
        notice = `悬浮层事件初始化失败：${String(error)}`;
      });

    return () => {
      disposed = true;
      if (moveTimer) clearTimeout(moveTimer);
      unlisteners.forEach((dispose) => dispose());
    };
  });

  async function setPanelOpen(open: boolean): Promise<void> {
    if (desktopBridgeAvailable) {
      await runCommand('overlay.set_open', { open });
    }
    panelOpen = open;
    notice = '';
    if (!open) {
      convertInput = null;
      settingsOpen = false;
      monitorOpen = false;
      editMode = false;
      trayOpen = false;
      editDrag = null;
      resizeDrag = null;
      editTarget = -1;
      editRemove = false;
    }
  }

  async function openSettings(): Promise<void> {
    try {
      await setPanelOpen(true);
      settingsOpen = true;
      monitorOpen = false;
    } catch (error) {
      notice = `打开设置失败：${String(error)}`;
    }
  }

  function startEdit(): void {
    draftLayout = [...discLayout];
    draftGeometry = { ...discGeometry };
    editMode = true;
    trayOpen = false;
    editDrag = null;
    resizeDrag = null;
    editTarget = -1;
    editRemove = false;
    notice = '';
  }

  function cancelEdit(): void {
    editMode = false;
    trayOpen = false;
    editDrag = null;
    resizeDrag = null;
    editTarget = -1;
    editRemove = false;
    notice = '';
  }

  function saveEdit(): void {
    const next = sanitizeDiscLayout(draftLayout);
    if (!next) return;
    discLayout = next;
    discGeometry = { ...draftGeometry };
    try {
      localStorage.setItem(DISC_LAYOUT_KEY, JSON.stringify(next));
      localStorage.setItem(DISC_GEOMETRY_KEY, JSON.stringify(draftGeometry));
    } catch { /* ignore */ }
    editMode = false;
    settingsOpen = false;
    trayOpen = false;
    editDrag = null;
    resizeDrag = null;
    editTarget = -1;
    editRemove = false;
    notice = `轮盘布局已保存（${next.length} 项）。`;
  }

  function restoreDefaultLayout(): void {
    draftLayout = [...DEFAULT_DISC_LAYOUT];
    draftGeometry = { ...DEFAULT_DISC_GEOMETRY };
    trayOpen = false;
  }

  function toggleTray(): void {
    trayOpen = !trayOpen;
  }

  function addDraftFunction(id: string): void {
    draftLayout = [...draftLayout, id];
    trayOpen = false;
  }

  function discardSector(index: number): void {
    if (draftLayout.length <= 1) return;
    draftLayout = draftLayout.filter((_, i) => i !== index);
  }

  function editPointerInfo(event: PointerEvent): { distance: number; angle: number } | null {
    if (!discSvg) return null;
    const rect = discSvg.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
    const x = ((event.clientX - rect.left) / rect.width) * VIEW;
    const y = ((event.clientY - rect.top) / rect.height) * VIEW;
    const dx = x - DCX;
    const dy = y - DCY;
    let angle = Math.atan2(dy, dx);
    if (angle < 0) angle += Math.PI * 2;
    return { distance: Math.hypot(dx, dy), angle };
  }

  function editIndexAt(angle: number): number {
    const count = draftLayout.length || 1;
    return Math.max(0, Math.min(count - 1, Math.floor(angle / ((Math.PI * 2) / count))));
  }

  function editPointerDown(event: PointerEvent, index: number): void {
    event.stopPropagation();
    editDrag = { pointerId: event.pointerId, index, startX: event.clientX, startY: event.clientY, active: false };
    editTarget = index;
    editRemove = false;
    // 捕获到 SVG 而非根节点：事件仍冒泡到 window，但松手落在窗口外也能收到 pointerup
    try { discSvg?.setPointerCapture(event.pointerId); } catch { /* 指针已失效时忽略 */ }
  }

  function editPointerMove(event: PointerEvent): void {
    if (!editDrag || editDrag.pointerId !== event.pointerId) return;
    if (!editDrag.active) {
      if (Math.hypot(event.clientX - editDrag.startX, event.clientY - editDrag.startY) <= 5) return;
      editDrag = { ...editDrag, active: true };
    }
    const info = editPointerInfo(event);
    editRemove = info !== null && info.distance <= viewRIn;
    editTarget = info && !editRemove && info.distance <= viewROut + 28 ? editIndexAt(info.angle) : -1;
  }

  function editPointerUp(event: PointerEvent): void {
    if (!editDrag || editDrag.pointerId !== event.pointerId) return;
    const drag = editDrag;
    editDrag = null;
    const target = editTarget;
    editTarget = -1;
    editRemove = false;
    if (!drag.active) return;
    const info = editPointerInfo(event);
    if (!info) return;
    // 拖到内圆里或拖出外圈：移除该扇区，其余扇区自动等分圆盘补位
    if (info.distance <= viewRIn || info.distance > viewROut + 28) {
      discardSector(drag.index);
      return;
    }
    if (target < 0 || target === drag.index) return;
    const next = [...draftLayout];
    const [moved] = next.splice(drag.index, 1);
    next.splice(target, 0, moved);
    draftLayout = next;
  }

  function resizePointerDown(event: PointerEvent, kind: 'inner' | 'outer'): void {
    event.stopPropagation();
    resizeDrag = { pointerId: event.pointerId, kind };
    editRemove = false;
    editTarget = -1;
    try { discSvg?.setPointerCapture(event.pointerId); } catch { /* 指针已失效时忽略 */ }
  }

  function applyRingRadius(kind: 'inner' | 'outer', distance: number): void {
    if (kind === 'inner') {
      const upper = Math.min(R_IN_MAX, draftGeometry.rOut - MIN_RING_WIDTH);
      const value = Math.min(upper, Math.max(R_IN_MIN, Math.round(distance)));
      draftGeometry = { ...draftGeometry, rIn: value };
    } else {
      const lower = Math.max(R_OUT_MIN, draftGeometry.rIn + MIN_RING_WIDTH);
      const value = Math.max(lower, Math.min(R_OUT_MAX, Math.round(distance)));
      draftGeometry = { ...draftGeometry, rOut: value };
    }
  }

  function resizePointerMove(event: PointerEvent): void {
    if (!resizeDrag || resizeDrag.pointerId !== event.pointerId) return;
    const info = editPointerInfo(event);
    if (!info) return;
    applyRingRadius(resizeDrag.kind, info.distance);
  }

  function resizeKeydown(event: KeyboardEvent, kind: 'inner' | 'outer'): void {
    if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
    event.preventDefault();
    event.stopPropagation();
    const base = kind === 'inner' ? draftGeometry.rIn : draftGeometry.rOut;
    applyRingRadius(kind, base + (event.key === 'ArrowUp' ? 2 : -2));
  }

  async function quitApp(): Promise<void> {
    if (!desktopBridgeAvailable) {
      notice = '浏览器预览无法退出 Orbit。';
      return;
    }
    try {
      await runCommand('app.quit');
    } catch (error) {
      notice = `退出失败：${String(error)}`;
    }
  }

  async function beginSetup(): Promise<void> {
    if (setupRunning) return;

    setupRunning = true;
    notice = '';
    try {
      await setPanelOpen(true);
      if (!desktopBridgeAvailable) {
        notice = '浏览器预览不执行系统设置。请在 Orbit 桌面端使用此功能。';
        return;
      }

      const result = await runCommand<SetupResult>('app.setup.complete');
      autostartConfigured = result.autostartConfigured;
      permissionStates = result.permissions;
      setupAttempted = result.autostartConfigured;
      notice = autostartConfigured ? '已启用登录时启动。' : '登录时启动尚未启用，请重试。';
    } catch (error) {
      notice = `一键设置失败：${String(error)}`;
    } finally {
      setupRunning = false;
    }
  }

  async function toggleAutostart(): Promise<void> {
    if (setupRunning) return;
    if (!desktopBridgeAvailable) {
      notice = '浏览器预览不修改系统设置。请在 Orbit 桌面端使用此功能。';
      return;
    }

    setupRunning = true;
    notice = '';
    try {
      const result = await runCommand<SetupResult>('app.autostart.set', {
        enabled: !autostartConfigured,
      });
      autostartConfigured = result.autostartConfigured;
      permissionStates = result.permissions;
      setupAttempted = true;
      notice = autostartConfigured ? '已启用登录时启动。' : '已关闭登录时启动。';
    } catch (error) {
      notice = `更新登录启动设置失败：${String(error)}`;
    } finally {
      setupRunning = false;
    }
  }

  async function captureScreenshot(): Promise<void> {
    if (captureRunning) return;
    if (!desktopBridgeAvailable) {
      notice = '浏览器预览不执行截图。请在 Orbit 桌面端使用此功能。';
      return;
    }

    captureRunning = true;
    notice = '';
    try {
      await runCommand('screenshot.capture', { mode: 'fullscreen' });
      notice = '全屏截图已复制到剪贴板。';
    } catch (error) {
      notice = `截图失败：${String(error)}`;
    } finally {
      captureRunning = false;
    }
  }

  async function beginRegionScreenshot(): Promise<void> {
    if (captureRunning) return;
    if (!desktopBridgeAvailable) {
      notice = '浏览器预览不执行截图。请在 Orbit 桌面端使用此功能。';
      return;
    }

    captureRunning = true;
    notice = '';
    try {
      await runCommand('region.start');
    } catch (error) {
      captureRunning = false;
      notice = `无法开始区域截图：${String(error)}`;
    }
  }

  function selectionPoint(event: PointerEvent): { x: number; y: number } {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    return {
      x: Math.max(0, Math.min(bounds.width, event.clientX - bounds.left)),
      y: Math.max(0, Math.min(bounds.height, event.clientY - bounds.top)),
    };
  }

  function handleSelectionPointerDown(event: PointerEvent): void {
    if (!selectingRegion || event.button !== 0) return;
    const point = selectionPoint(event);
    selectionStart = point;
    selectionBounds = { left: point.x, top: point.y, width: 0, height: 0 };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function updateRegionSelection(event: PointerEvent): void {
    if (!selectingRegion || !selectionStart) return;
    const point = selectionPoint(event);
    selectionBounds = {
      left: Math.min(selectionStart.x, point.x),
      top: Math.min(selectionStart.y, point.y),
      width: Math.abs(point.x - selectionStart.x),
      height: Math.abs(point.y - selectionStart.y),
    };
  }

  async function finishRegionScreenshot(event: PointerEvent): Promise<void> {
    if (!selectingRegion || !selectionStart || selectionMonitorId === null) return;
    updateRegionSelection(event);
    const bounds = selectionBounds;
    if (!bounds || bounds.width < 1 || bounds.height < 1) {
      await cancelRegionScreenshot();
      return;
    }
    selectingRegion = false;

    const scale = window.devicePixelRatio;
    const request = {
      monitorId: selectionMonitorId,
      x: Math.round(bounds.left * scale),
      y: Math.round(bounds.top * scale),
      width: Math.max(1, Math.round(bounds.width * scale)),
      height: Math.max(1, Math.round(bounds.height * scale)),
    };
    selectionStart = null;
    selectionBounds = null;
    try {
      const response = await runCommand<CommandResponse>('region.finish', request);
      await emit('region-capture-result', { response });
    } catch (error) {
      try {
        await runCommand('region.cancel');
      } catch (restoreError) {
        await emit('region-capture-result', {
          error: `${String(error)}；恢复 Orbit 主窗口失败：${String(restoreError)}`,
        });
        return;
      }
      await emit('region-capture-result', { error: String(error) });
    }
  }

  async function cancelRegionScreenshot(): Promise<void> {
    if (!selectingRegion) return;
    selectingRegion = false;
    selectionStart = null;
    selectionBounds = null;
    try {
      await runCommand('region.cancel');
      await emit('region-capture-result', { cancelled: true });
    } catch (error) {
      await emit('region-capture-result', { error: `取消区域截图失败：${String(error)}` });
    }
  }

  function handleKeyDown(event: KeyboardEvent): void {
    if (selectingRegion && event.key === 'Escape') {
      event.preventDefault();
      void cancelRegionScreenshot();
      return;
    }
    if (panelOpen && event.key === 'Escape') {
      event.preventDefault();
      if (editMode) {
        cancelEdit();
        return;
      }
      void setPanelOpen(false);
    }
  }

  function handlePointerDown(event: PointerEvent): void {
    if (event.button !== 0) return;

    dragStart = null;
    suppressClick = event.detail === 2;
    if (event.detail === 2) {
      event.preventDefault();
      return;
    }
    dragStart = { pointerId: event.pointerId, x: event.screenX, y: event.screenY };
  }

  function handlePointerMove(event: PointerEvent): void {
    // 编辑模式：拖拽状态在窗口级跟踪，避免指针捕获把事件重定向到根节点
    if (resizeDrag) {
      resizePointerMove(event);
      return;
    }
    if (editDrag) {
      editPointerMove(event);
      return;
    }
    if (!dragStart || dragStart.pointerId !== event.pointerId) return;
    if (Math.hypot(event.screenX - dragStart.x, event.screenY - dragStart.y) <= 4) return;

    dragStart = null;
    suppressClick = true;
    if (!desktopBridgeAvailable) return;

    void getCurrentWindow()
      .startDragging()
      .catch((error: unknown) => {
        notice = `悬浮球拖动失败：${String(error)}`;
      });
  }

  function handlePointerUp(event: PointerEvent): void {
    if (resizeDrag) {
      if (resizeDrag.pointerId === event.pointerId) resizeDrag = null;
      return;
    }
    if (editDrag) {
      editPointerUp(event);
      return;
    }
    if (dragStart?.pointerId === event.pointerId) dragStart = null;
  }

  async function handleOrbClick(): Promise<void> {
    if (suppressClick) {
      suppressClick = false;
      return;
    }
    if (!setupAttempted) {
      await beginSetup();
      return;
    }
    try {
      await setPanelOpen(!panelOpen);
    } catch (error) {
      notice = `展开 Orbit 覆盖层失败：${String(error)}`;
    }
  }

  // 圆盘展开时中心球既用于拖动窗口，也用于点击收起：拖拽后的 click 直接吞掉
  function handleExpandedOrbClick(): void {
    if (suppressClick) {
      suppressClick = false;
      return;
    }
    void setPanelOpen(false);
  }

  async function handleContextMenu(event: MouseEvent): Promise<void> {
    event.preventDefault();
    if (selectingRegion) {
      await cancelRegionScreenshot();
      return;
    }
    await openSettings();
  }

  async function closePanel(): Promise<void> {
    try {
      await setPanelOpen(false);
    } catch (error) {
      notice = `收起覆盖层失败：${String(error)}`;
    }
  }

  // 剪贴板窗口不参与悬浮球交互，避免全局拖拽 / 右键菜单干扰面板
  const mainContextMenu = (event: MouseEvent) => { if (!isClipboardWindow) void handleContextMenu(event); };
  const mainPointerMove = (event: PointerEvent) => { if (!isClipboardWindow) handlePointerMove(event); };
  const mainPointerUp = (event: PointerEvent) => { if (!isClipboardWindow) handlePointerUp(event); };
  const mainKeyDown = (event: KeyboardEvent) => { if (!isClipboardWindow) handleKeyDown(event); };
</script>

<svelte:head>
  <title>Orbit · 桌面覆盖层</title>
  <meta name="description" content="Orbit 桌面覆盖层与游戏状态浮窗" />
</svelte:head>

<svelte:window
  oncontextmenu={mainContextMenu}
  onpointermove={mainPointerMove}
  onpointerup={mainPointerUp}
  onpointercancel={mainPointerUp}
  onkeydown={mainKeyDown}
/>

{#if isClipboardWindow}
  <ClipboardPanel />
{:else if selectingRegion}
  <main
    class="region-selection-shell"
    aria-label="拖动选择截图区域，按 Escape 取消"
    onpointerdown={handleSelectionPointerDown}
    onpointermove={updateRegionSelection}
    onpointerup={finishRegionScreenshot}
    onpointercancel={cancelRegionScreenshot}
    oncontextmenu={(event) => {
      event.preventDefault();
      void cancelRegionScreenshot();
    }}
  >
    <p class="selection-instructions">拖动选择截图区域 <span>·</span> Esc 取消</p>
    {#if selectionBounds}
      <div
        class="selection-rectangle"
        style:left="{selectionBounds.left}px"
        style:top="{selectionBounds.top}px"
        style:width="{selectionBounds.width}px"
        style:height="{selectionBounds.height}px"
      ></div>
    {/if}
  </main>
{:else}
<div
  class:expanded={panelOpen}
  class:top-docked={dockPosition === 'top'}
  class:left-docked={dockPosition === 'left'}
  class:right-docked={dockPosition === 'right'}
  class="app-shell"
>
  {#if recordingActive}
    <div class="rec-control">
      <span class="rec-timer">{formatRecTime(recordElapsedMs)}</span>
      <div class="rec-buttons">
        <button class="rec-btn" type="button" onclick={togglePause} aria-label={recordingPaused ? '继续录制' : '暂停录制'}>{recordingPaused ? '▶' : '⏸'}</button>
        <button class="rec-btn stop" type="button" onclick={stopRecord} aria-label="停止并保存">⏹</button>
      </div>
    </div>
  {:else if panelOpen}
    {#if settingsOpen && !editMode}
      <DiscSettings
        autostartConfigured={autostartConfigured}
        setupRunning={setupRunning}
        {notice}
        layoutCount={discLayout.length}
        recordDir={recordDir}
        onpickrecorddir={pickRecordDir}
        onclearrecorddir={clearRecordDir}
        onedit={startEdit}
        ontoggleautostart={() => void toggleAutostart()}
        onquit={() => void quitApp()}
        onclose={() => { settingsOpen = false; }}
      />
    {:else}
    <!-- SVG 圆盘扇区；鼠标驱动的悬浮层，键盘收起由全局 Esc 处理 -->
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <svg
      class="disc open"
      class:editing={editMode}
      class:removing={editMode && editRemove}
      bind:this={discSvg}
      viewBox={`0 0 ${VIEW} ${VIEW}`}
      onclick={(e) => {
        // 点击圆盘外的空白区收起（扇区自身会 stopPropagation）
        if (e.target === e.currentTarget && !editMode) void setPanelOpen(false);
      }}
    >
      {#each viewActions as a, i}
        {@const a0 = (i / sectorCount) * Math.PI * 2 + GAP / 2}
        {@const a1 = ((i + 1) / sectorCount) * Math.PI * 2 - GAP / 2}
        {@const mid = (a0 + a1) / 2}
        {@const [tx, ty] = pt((viewRIn + viewROut) / 2, mid)}
        <g
          class="seg-group"
          class:flash={flashIndex === i}
          class:edit-highlight={editMode && editTarget === i}
          role="button"
          tabindex="0"
          aria-label={a.label}
          style="--d: {(i * 0.032).toFixed(3)}s"
          onpointerdown={editMode
            ? (e) => editPointerDown(e, i)
            : (e) => { e.stopPropagation(); dragStart = null; }}
          onclick={(e) => { e.stopPropagation(); if (!editMode) segClick(i); }}
          onkeydown={(e) => {
            if (!editMode && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); segClick(i); }
          }}
        >
          <path class="seg-fill" d={sectorPath(viewRIn, viewROut, a0, a1)} />
          <text
            class="seg-label"
            x={tx}
            y={ty}
            text-anchor="middle"
            dominant-baseline="middle"
          >{a.label}</text>
        </g>
      {/each}

      {#if editMode}
        <circle class="edit-ring" cx={DCX} cy={DCY} r={viewRIn} />
        <circle class="edit-ring" cx={DCX} cy={DCY} r={viewROut} />
        <g
          class="edit-knob"
          role="slider"
          tabindex="0"
          aria-label="内圆大小"
          aria-valuemin={R_IN_MIN}
          aria-valuemax={R_IN_MAX}
          aria-valuenow={Math.round(viewRIn)}
          onpointerdown={(e) => resizePointerDown(e, 'inner')}
          onkeydown={(e) => resizeKeydown(e, 'inner')}
        >
          <circle cx={DCX} cy={DCY - viewRIn} r="7" />
          <title>拖动或按 ↑↓ 调整内圆</title>
        </g>
        <text class="edit-ring-value" x={DCX + 14} y={DCY - viewRIn} dominant-baseline="middle">{Math.round(viewRIn)}</text>
        <g
          class="edit-knob"
          role="slider"
          tabindex="0"
          aria-label="外圈大小"
          aria-valuemin={R_OUT_MIN}
          aria-valuemax={R_OUT_MAX}
          aria-valuenow={Math.round(viewROut)}
          onpointerdown={(e) => resizePointerDown(e, 'outer')}
          onkeydown={(e) => resizeKeydown(e, 'outer')}
        >
          <circle cx={DCX} cy={DCY - viewROut} r="7" />
          <title>拖动或按 ↑↓ 调整外圈</title>
        </g>
        <text class="edit-ring-value" x={DCX + 14} y={DCY - viewROut} dominant-baseline="middle">{Math.round(viewROut)}</text>
      {/if}
    </svg>

    <!-- 中心悬浮球 -->
    <div class="orb-center">
      {#if editMode}
        <button
          class="orb-button active"
          type="button"
          aria-label="添加功能"
          title="点击选择要加回的功能"
          onpointerdown={(e) => e.stopPropagation()}
          onclick={(e) => { e.stopPropagation(); toggleTray(); }}
        >
          <OrbGalaxy bind:this={galaxyRef} />
          <span class="edit-add-glyph" class:remove={editRemove} aria-hidden="true">{editRemove ? '✕' : '+'}</span>
        </button>
      {:else}
        <button
          class="orb-button active"
          class:hovered={orbHovered}
          type="button"
          aria-label="移动或收起圆盘"
          title="Orbit · 拖动移动，点击收起"
          onpointerdown={handlePointerDown}
          onclick={(e) => { e.stopPropagation(); handleExpandedOrbClick(); }}
          onmouseenter={() => { orbHovered = true; galaxyRef?.setHover(true); }}
          onmouseleave={() => { orbHovered = false; galaxyRef?.setHover(false); galaxyRef?.setParallax(0, 0); }}
          onmousemove={handleOrbParallax}
        >
          <OrbGalaxy bind:this={galaxyRef} />
          <span class="orb-wordmark" aria-hidden="true">Orbit</span>
          <span class="orb-ring" aria-hidden="true"></span>
          <span class="orb-ring slow" aria-hidden="true"></span>
        </button>
        {#if notice}<p class="disc-notice" role="status">{notice}</p>{/if}
      {/if}
    </div>

    {#if editMode}
      <p class="edit-hint" role="status">
        {editRemove ? '松手删除该扇区' : `拖动排序 · 拖到中心或拖出圆盘删除 · ${draftLayout.length} 项`}
      </p>
      <div class="edit-bar">
        <button class="settings-btn ghost" type="button" onclick={restoreDefaultLayout}>恢复默认</button>
        <button class="settings-btn" type="button" onclick={cancelEdit}>取消</button>
        <button class="settings-btn primary" type="button" onclick={saveEdit}>保存</button>
      </div>
      {#if trayOpen}
        <div class="add-tray" role="dialog" aria-label="添加功能">
          <p class="convert-title">添加功能</p>
          {#if unusedActions.length === 0}
            <p class="add-tray-empty">全部功能已在圆盘上</p>
          {:else}
            {#each unusedActions as f (f.id)}
              <button type="button" class="convert-option" onclick={() => addDraftFunction(f.id)}>
                {f.icon} {f.label}
              </button>
            {/each}
          {/if}
          <button type="button" class="convert-option cancel" onclick={() => { trayOpen = false; }}>取消</button>
        </div>
      {/if}
    {:else if convertInput}
      <div class="convert-picker" role="dialog" aria-label="选择转换目标格式">
        <p class="convert-title">转换为…</p>
        {#each CONVERT_FORMATS as f}
          <button
            type="button"
            class="convert-option"
            disabled={convertBusy}
            onclick={() => void chooseConvertFormat(f.id)}
          >{f.label}</button>
        {/each}
        <button
          type="button"
          class="convert-option cancel"
          disabled={convertBusy}
          onclick={cancelConvert}
        >取消</button>
      </div>
    {/if}
    {#if monitorOpen}
      <SystemMonitor onclose={() => { monitorOpen = false; }} />
    {/if}
    {/if}
  {:else if dockPosition === 'top'}
    <button
      class="notch-button"
      type="button"
      aria-label="展开 Orbit 灵动岛"
      title="Orbit · 点击展开"
      onpointerdown={handlePointerDown}
      onclick={handleOrbClick}
    >
      <span class="notch-clock">{clockNow}</span>
      {#if dashInfo}
        <span class="notch-sep"></span>
        <span class="notch-stats">
          <span class="notch-stat"><small>处理器</small>{Math.round(dashInfo.cpuPercent)}%</span>
          <span class="notch-stat"><small>内存</small>{Math.round(dashInfo.memoryUsedPercent)}%</span>
          {#if dashInfo.temperatureC != null}
            <span class="notch-stat" title="CPU 温度"><small>温度</small>{Math.round(dashInfo.temperatureC)}°C</span>
          {/if}
        </span>
      {/if}
    </button>
  {:else if dockPosition === 'left' || dockPosition === 'right'}
    <button
      class="edge-button"
      type="button"
      aria-label="展开 Orbit 侧边栏"
      title="Orbit · 点击展开侧边栏"
      onpointerdown={handlePointerDown}
      onclick={handleOrbClick}
    >
      <span class="orb-glyph" aria-hidden="true">O</span>
      <span class="edge-indicator" aria-hidden="true"></span>
    </button>
  {:else}
    <button
      class="orb-button"
      class:hovered={orbHovered}
      type="button"
      aria-label="展开 Orbit 悬浮岛"
      title="Orbit · 点击展开"
      onpointerdown={handlePointerDown}
      onclick={handleOrbClick}
      onmouseenter={() => { orbHovered = true; galaxyRef?.setHover(true); }}
      onmouseleave={() => { orbHovered = false; galaxyRef?.setHover(false); galaxyRef?.setParallax(0, 0); }}
      onmousemove={handleOrbParallax}
    >
      <OrbGalaxy bind:this={galaxyRef} />
      <span class="orb-wordmark" aria-hidden="true">Orbit</span>
      <span class="orb-ring" aria-hidden="true"></span>
      <span class="orb-ring slow" aria-hidden="true"></span>
    </button>
  {/if}
</div>
{/if}
