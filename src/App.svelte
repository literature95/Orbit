<script lang="ts">
  import { invoke, isTauri } from '@tauri-apps/api/core';
  import { emit, listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount } from 'svelte';
  import type { CommandRequest } from '../crates/orbit-core/bindings/CommandRequest';
  import type { CommandResponse } from '../crates/orbit-core/bindings/CommandResponse';

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

  onMount(() => {
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
>
  {#if panelOpen}
    <main class="island-panel">
      <header class="panel-header">
        <div class="brand-lockup">
          <span class="brand-mark" aria-hidden="true">O</span>
          <div>
            <p class="eyebrow">ORBIT · DESKTOP OVERLAY</p>
            <h1>{dockPosition === 'top' ? '灵动岛' : dockPosition === 'floating' ? 'Orbit' : 'Orbit 侧边栏'}</h1>
          </div>
        </div>
        <button class="close-button" type="button" aria-label="收起覆盖层" onclick={closePanel}>×</button>
      </header>

      <section class="telemetry-card" aria-label="系统与游戏状态">
        <div class="telemetry-item">
          <span>游戏帧率</span>
          <strong>{metrics.gameFps === null ? '--' : metrics.gameFps}<small> FPS</small></strong>
          <small class="metric-note">通用游戏接口未接入</small>
        </div>
        <div class="telemetry-item">
          <span>系统 CPU</span>
          <strong>{Math.round(metrics.cpuUsage)}<small>%</small></strong>
          <small class="metric-note">整机总占用</small>
        </div>
        <div class="telemetry-item">
          <span>CPU 温度</span>
          <strong>{formatCpuTemperature(metrics.cpuTemperature)}</strong>
          <small class="metric-note">{metrics.cpuTemperature === null ? '传感器不可用' : '传感器读数'}</small>
        </div>
      </section>

      <div class="action-row">
        <button
          class="configure-button"
          type="button"
          disabled={setupRunning}
          onclick={toggleAutostart}
        >
          <span class="button-orbit" aria-hidden="true">O</span>
          <span>{setupRunning ? '正在更新…' : autostartConfigured ? '关闭登录启动' : '启用登录启动'}</span>
        </button>
        <button
          class="capture-button"
          type="button"
          disabled={captureRunning}
          onclick={captureScreenshot}
        >
          {captureRunning ? '正在截图…' : '全屏截图'}
        </button>
        <button
          class="capture-button"
          type="button"
          disabled={captureRunning}
          onclick={beginRegionScreenshot}
        >
          区域截图
        </button>
      </div>

      <p class="privacy-note">
        屏幕录制：{permissionStates['screen-capture'] === 'unknown' ? '未检测' : '未知'}；辅助功能：{permissionStates['accessibility-input'] === 'unknown' ? '未检测' : '未知'}。独占全屏覆盖与游戏 FPS 暂不保证。
      </p>
      {#if notice}<p class="notice" role="status">{notice}</p>{/if}
    </main>
  {:else if dockPosition === 'top'}
    <button
      class="notch-button"
      type="button"
      aria-label="展开 Orbit 灵动岛"
      title="Orbit · 点击展开"
      onpointerdown={handlePointerDown}
      onclick={handleOrbClick}
    >
      <span class="notch-brand">O</span>
      <span class="notch-metric"><small>FPS</small> --</span>
      <span class="notch-metric"><small>CPU</small> {Math.round(metrics.cpuUsage)}%</span>
      <span class="notch-metric"><small>TEMP</small> {formatCpuTemperature(metrics.cpuTemperature)}</span>
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
      type="button"
      aria-label="展开 Orbit 悬浮岛"
      title="Orbit · 点击展开"
      onpointerdown={handlePointerDown}
      onclick={handleOrbClick}
    >
      <span class="orb-glyph" aria-hidden="true">O</span>
      <span class="orb-status" aria-hidden="true"></span>
    </button>
  {/if}
</div>
{/if}
