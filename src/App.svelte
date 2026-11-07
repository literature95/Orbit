<script lang="ts">
  import { invoke, isTauri } from '@tauri-apps/api/core';
  import { emit, listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount } from 'svelte';
  import type { CommandRequest } from '../crates/orbit-core/bindings/CommandRequest';
  import type { CommandResponse } from '../crates/orbit-core/bindings/CommandResponse';
  import OrbGalaxy from './OrbGalaxy.svelte';

  type DockPosition = 'floating' | 'left' | 'right' | 'top';
  type OverlaySnapshot = { dock: DockPosition; expanded: boolean };
  type SetupResult = {
    autostartConfigured: boolean;
    permissions: Record<string, 'unknown'>;
  };
  type SystemMetrics = {
    cpuUsage: number;
    cpuTemperature: number | null;
    gameFps: number | null;
  };
  type SystemInfo = {
    osName: string;
    osVersion: string;
    screenWidth: number;
    screenHeight: number;
    scaleFactor: number;
    totalMemoryMb: number;
    availableMemoryMb: number;
  };
  type RegionSelectionStart = { monitorId: number };
  type RegionBounds = { left: number; top: number; width: number; height: number };

  const desktopBridgeAvailable = isTauri();
  const emptyMetrics: SystemMetrics = { cpuUsage: 0, cpuTemperature: null, gameFps: null };
  let dockPosition: DockPosition = 'floating';
  let panelOpen = false;
  let setupRunning = false;
  let captureRunning = false;
  let autostartConfigured = false;
  let setupAttempted = false;
  let permissionStates: SetupResult['permissions'] = {};
  let metrics = emptyMetrics;
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

  // ============ 圆盘几何 ============
  // DCX/DCY = SVG viewBox 中心 = 200/200（窗口 400×400）
  // 悬浮球半径 = 41px（orb-button 宽 82px 的一半）
  // 扇区内圈贴住球外圈，外圈紧凑不太长
  const SEGS = 12;
  const DCX = 200, DCY = 200;
  const ORB_RADIUS = 41;
  const R_IN = ORB_RADIUS + 6;   // 47 — 贴住球外圈
  const R_OUT = 112;             // 紧凑，扇区宽度 = 65
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
    { id: 'setup', icon: '⚙️', label: '设置', handler: toggleAutostart },
    { id: 'close', icon: '✕', label: '关闭', handler: () => { void setPanelOpen(false); } },
  ];

  function segClick(idx: number) {
    flashIndex = idx;
    setTimeout(() => { flashIndex = -1; void actions[idx].handler(); }, 180);
  }

  // ============ 灵动岛仪表盘 ============
  type DashInfo = {
    cpuPercent: number;
    memoryUsedPercent: number;
    memoryUsedMb: number;
    memoryTotalMb: number;
    uptimeSecs: number;
    timeNow: string;
    clipboardCount: number;
  };
  let dashInfo: DashInfo | null = null;

  function formatUptime(secs: number) {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = secs % 60;
    if (h > 0) return `${h}h${String(m).padStart(2, '0')}m`;
    if (m > 0) return `${m}m${String(s).padStart(2, '0')}s`;
    return `${s}s`;
  }

  function formatClock() {
    const d = new Date();
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}:${String(d.getSeconds()).padStart(2, '0')}`;
  }

  let clockNow = formatClock();
  $: if (dockPosition === 'top' || dockPosition === 'floating') {
    // 每秒刷新时钟
  }

  // ============ 主题系统 ============
  const THEMES = ['cosmic-violet', 'pure-black', 'cyber-blue', 'mono-light'] as const;
  const THEME_LABELS: Record<(typeof THEMES)[number], string> = {
    'cosmic-violet': '深空紫',
    'pure-black': '纯黑极简',
    'cyber-blue': '赛博蓝',
    'mono-light': '极简白',
  };
  let currentTheme: (typeof THEMES)[number] = 'cosmic-violet';

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

  // ============ 系统监控 ============
  let monitorInfo: SystemInfo | null = null;
  let recordingActive = false;
  async function toggleRecord() {
    if (!desktopBridgeAvailable) { notice = '请在桌面端使用录屏。'; return; }
    if (!recordingActive) {
      try {
        const path = await invoke<string>('record_start');
        recordingActive = true;
        notice = `⏺ 录屏中… 保存到：${path}`;
      } catch (e) { notice = `录屏启动失败：${String(e)}`; }
    } else {
      try {
        const path = await invoke<string | null>('record_stop');
        recordingActive = false;
        notice = path ? `✅ 录屏已保存：${path}` : '✅ 录屏已停止';
      } catch (e) { notice = `录屏停止失败：${String(e)}`; }
    }
  }

  async function runOcr() {
    if (!desktopBridgeAvailable) { notice = '请在桌面端使用 OCR。'; return; }
    notice = '🔍 正在截屏+识别…';
    try {
      const text = await invoke<string>('ocr_current_screen');
      await invoke('clip_write', { text });
      notice = `✅ OCR 完成（${text.length}字），已写回剪贴板：${text.slice(0, 30)}…`;
    } catch (e) { notice = `OCR 失败：${String(e)}`; }
  }

  let kbRecording = false;
  async function toggleKbRecord() {
    if (!desktopBridgeAvailable) { notice = '请在桌面端使用。'; return; }
    if (!kbRecording) {
      try { await invoke('kb_record_start'); kbRecording = true; notice = '🎮 键鼠录制中…（ESC 无法停止，重启应用）'; }
      catch (e) { notice = `录制启动失败：${String(e)}`; }
    } else {
      try { await invoke('kb_record_stop'); kbRecording = false; notice = '🎮 录制已停止'; }
      catch (e) { notice = `停止失败：${String(e)}`; }
    }
  }

  async function runConvert() {
    if (!desktopBridgeAvailable) { notice = '请在桌面端使用。'; return; }
    try {
      const { open, save } = await import('@tauri-apps/plugin-dialog');
      const input = await open({ title: '选择要转换的文件', multiple: false });
      if (!input || typeof input !== 'string') { notice = '已取消'; return; }

      // 问目标格式
      const formats = ['mp3', 'gif', 'webp', 'mp4', 'wav'];
      // 简化：直接让用户选扩展名（这里 hardcode 转 mp3 演示完整链路）
      const targetFmt = 'mp3';

      const outPath = await save({
        title: `保存为 .${targetFmt}`,
        defaultPath: input.replace(/\.[^.]+$/, `.${targetFmt}`),
      });
      if (!outPath) { notice = '已取消'; return; }

      notice = `🔄 正在转换 → ${outPath}`;
      const result = await invoke<string>('convert_format', {
        inputPath: input,
        outputPath: outPath,
        format: targetFmt,
      });
      notice = `✅ 转换完成：${result}`;
    } catch (e) {
      notice = `转换失败：${String(e)}（需要系统安装 FFmpeg）`;
    }
  }

  function showMcp() {
    notice = '🔌 MCP sidecar 待接入（需要打包 mcp server 二进制）';
  }
  async function showSystemMonitor() {
    if (!desktopBridgeAvailable) { notice = '请在 Orbit 桌面端使用系统监控。'; return; }
    try {
      monitorInfo = await invoke<SystemInfo>('get_system_info');
      notice = `${monitorInfo.screenWidth}×${monitorInfo.screenHeight} · ${monitorInfo.totalMemoryMb}MB RAM`;
    } catch (error) {
      notice = `系统信息获取失败：${String(error)}`;
    }
  }
  // ============ 剪贴板历史 ============
  interface ClipItem { text: string; preview: string; createdAt: number; }
  let clipHistory: ClipItem[] = [];
  let clipPollTimer: ReturnType<typeof setInterval> | null = null;
  let dashPollTimer: ReturnType<typeof setInterval> | null = null;
  let clockTimer: ReturnType<typeof setInterval> | null = null;

  async function pollDashInfo() {
    if (!desktopBridgeAvailable) {
      dashInfo = {
        cpuPercent: 0, memoryUsedPercent: 0, memoryUsedMb: 0, memoryTotalMb: 0,
        uptimeSecs: 0, timeNow: '', clipboardCount: clipHistory.length,
      };
      return;
    }
    try {
      const d = await invoke<DashInfo>('get_dash_info');
      d.clipboardCount = clipHistory.length;
      dashInfo = d;
    } catch { /* 忽略 */ }
  }

  function loadClipHistory() {
    try { clipHistory = JSON.parse(localStorage.getItem('orbit.clipHistory') || '[]'); } catch { clipHistory = []; }
  }
  function saveClipHistory() {
    try { localStorage.setItem('orbit.clipHistory', JSON.stringify(clipHistory.slice(0, 50))); } catch {}
  }

  async function pollClipboard() {
    if (!desktopBridgeAvailable) return;
    try {
      const text = await invoke<string>('clip_read');
      if (text && text.trim()) {
        const last = clipHistory[0];
        if (!last || last.text !== text) {
          clipHistory.unshift({
            text,
            preview: text.length > 60 ? text.slice(0, 60) + '…' : text.replace(/\n/g, ' '),
            createdAt: Date.now(),
          });
          clipHistory = clipHistory.slice(0, 50);
          saveClipHistory();
        }
      }
    } catch {}
  }

  async function readClipboard() {
    if (!desktopBridgeAvailable) { notice = '请在桌面端使用。'; return; }
    try {
      const text = await invoke<string>('clip_read');
      if (text) { notice = `📋 当前剪贴板（${text.length}字）：${text.slice(0, 40)}…`; }
      else { notice = '剪贴板为空。'; }
    } catch (error) { notice = `读取失败：${String(error)}`; }
  }

  async function writeClipboard(item: ClipItem) {
    if (!desktopBridgeAvailable) return;
    try { await invoke('clip_write', { text: item.text }); notice = '已写回剪贴板'; }
    catch (error) { notice = `写入失败：${String(error)}`; }
  }

  function showClipHistory() {
    if (clipHistory.length === 0) { notice = '剪贴板历史为空（每 3 秒自动轮询）。'; return; }
    notice = `📋 历史 ${clipHistory.length} 条 · 最新：${clipHistory[0].preview}`;
  }

  // 初始化：从 localStorage 读主题
  onMount(() => {
    loadClipHistory();
    pollClipboard(); // 立即读一次
    clipPollTimer = setInterval(pollClipboard, 3000); // 每 3 秒轮询
    pollDashInfo();
    dashPollTimer = setInterval(pollDashInfo, 2000);
    clockNow = formatClock();
    clockTimer = setInterval(() => { clockNow = formatClock(); }, 1000);
    try {
      const saved = localStorage.getItem('orbit.theme') as (typeof THEMES)[number] | null;
      if (saved && THEMES.includes(saved)) applyTheme(saved);
    } catch { /* ignore */ }
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
          void invoke('cancel_region_selection').then(
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
      listen('orb-setup-request', () => void beginSetup()),
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
      listen<SystemMetrics>('system-metrics', ({ payload }) => {
        metrics = payload;
      }),
      getCurrentWindow().onMoved(() => {
        if (moveTimer) clearTimeout(moveTimer);
        moveTimer = setTimeout(() => {
          void invoke<OverlaySnapshot>('dock_overlay')
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
        void invoke<SetupResult>('get_setup_status')
          .then((result) => {
            autostartConfigured = result.autostartConfigured;
            setupAttempted = result.autostartConfigured;
            permissionStates = result.permissions;
          })
          .catch((error: unknown) => {
            notice = `读取设置状态失败：${String(error)}`;
          });
        void invoke<OverlaySnapshot>('dock_overlay')
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
      await invoke('set_overlay_open', { open });
    }
    panelOpen = open;
    notice = '';
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

      const result = await invoke<SetupResult>('complete_first_run_setup');
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
      const result = await invoke<SetupResult>('set_autostart_enabled', {
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
    const request: CommandRequest = {
      commandId: 'screenshot.capture',
      params: { mode: 'fullscreen' },
    };
    try {
      const response = await invoke<CommandResponse>('execute_command', { request });
      if (response.status !== 'ok') {
        throw new Error(response.error ?? '截图命令执行失败。');
      }
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
      await invoke('start_region_selection');
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
      const response = await invoke<CommandResponse>('finish_region_selection', request);
      await emit('region-capture-result', { response });
    } catch (error) {
      try {
        await invoke('cancel_region_selection');
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
      await invoke('cancel_region_selection');
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

  async function handleContextMenu(event: MouseEvent): Promise<void> {
    event.preventDefault();
    if (selectingRegion) {
      await cancelRegionScreenshot();
      return;
    }
    if (!desktopBridgeAvailable) {
      notice = '原生右键菜单仅在桌面端可用。';
      panelOpen = true;
      return;
    }
    try {
      await invoke('show_orb_context_menu');
    } catch (error) {
      notice = `打开菜单失败：${String(error)}`;
      try {
        await setPanelOpen(true);
      } catch (panelError) {
        notice += `；展开覆盖层失败：${String(panelError)}`;
      }
    }
  }

  async function closePanel(): Promise<void> {
    try {
      await setPanelOpen(false);
    } catch (error) {
      notice = `收起覆盖层失败：${String(error)}`;
    }
  }

  function formatCpuTemperature(value: number | null): string {
    return value === null ? '--' : `${Math.round(value)}°`;
  }
</script>

<svelte:head>
  <title>Orbit · 桌面覆盖层</title>
  <meta name="description" content="Orbit 桌面覆盖层与游戏状态浮窗" />
</svelte:head>

<svelte:window
  oncontextmenu={handleContextMenu}
  onpointermove={handlePointerMove}
  onpointerup={handlePointerUp}
  onpointercancel={handlePointerUp}
  onkeydown={handleKeyDown}
/>

{#if selectingRegion}
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
  onclick={(e) => {
    // 点击 app-shell 空白处（非 disc 内部）收起
    if (panelOpen && (e.target as HTMLElement).classList?.contains('app-shell')) {
      void setPanelOpen(false);
    }
  }}
>
  {#if panelOpen}
    <!-- SVG 圆盘扇区 -->
    <svg class="disc open" viewBox="0 0 400 400">
      {#each actions as a, i}
        {@const a0 = (i / SEGS) * Math.PI * 2 + GAP / 2}
        {@const a1 = ((i + 1) / SEGS) * Math.PI * 2 - GAP / 2}
        {@const mid = (a0 + a1) / 2}
        {@const [tx, ty] = pt((R_IN + R_OUT) / 2, mid)}
        <g
          class="seg-group"
          class:flash={flashIndex === i}
          style="--d: {(i * 0.032).toFixed(3)}s"
          onclick={(e) => { e.stopPropagation(); segClick(i); }}
        >
          <path class="seg-fill" d={sectorPath(R_IN, R_OUT, a0, a1)} />
          <text
            class="seg-label"
            x={tx}
            y={ty}
            text-anchor="middle"
            dominant-baseline="middle"
          >{a.label}</text>
        </g>
      {/each}
    </svg>

    <!-- 中心悬浮球 -->
    <div class="orb-center">
      <button
        class="orb-button active"
        class:hovered={orbHovered}
        type="button"
        aria-label="关闭圆盘"
        title="Orbit · 点击收起"
        onpointerdown={(e) => e.stopPropagation()}
        onclick={(e) => { e.stopPropagation(); void setPanelOpen(false); }}
        onmouseenter={() => { orbHovered = true; galaxyRef?.setHover(true); }}
        onmouseleave={() => { orbHovered = false; galaxyRef?.setHover(false); }}
      >
        <OrbGalaxy bind:this={galaxyRef} />
        <span class="orb-ring" aria-hidden="true"></span>
        <span class="orb-ring slow" aria-hidden="true"></span>
      </button>
      {#if notice}<p class="disc-notice" role="status">{notice}</p>{/if}
    </div>
  {:else if dockPosition === 'top'}
    <button
      class="notch-button"
      type="button"
      aria-label="展开 Orbit 灵动岛"
      title="Orbit · 点击展开"
      onpointerdown={handlePointerDown}
      onclick={handleOrbClick}
    >
      <span class="notch-metric"><small>⏱</small>{clockNow}</span>
      <span class="notch-sep"></span>
      <span class="notch-metric"><small>CPU</small>{dashInfo ? Math.round(dashInfo.cpuPercent) : '--'}%</span>
      <span class="notch-metric"><small>MEM</small>{dashInfo ? Math.round(dashInfo.memoryUsedPercent) : '--'}%</span>
      <span class="notch-sep"></span>
      <span class="notch-metric"><small>⟳</small>{dashInfo ? formatUptime(dashInfo.uptimeSecs) : '--'}</span>
      <span class="notch-metric"><small>📋</small>{dashInfo ? dashInfo.clipboardCount : clipHistory.length}</span>
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
      onmouseleave={() => { orbHovered = false; galaxyRef?.setHover(false); }}
    >
      <OrbGalaxy bind:this={galaxyRef} />
      <span class="orb-ring" aria-hidden="true"></span>
      <span class="orb-ring slow" aria-hidden="true"></span>
    </button>
  {/if}
</div>
{/if}
