<script lang="ts">
  import { invoke, isTauri } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount } from 'svelte';

  type DockPosition = 'floating' | 'left' | 'right' | 'top';
  type OverlaySnapshot = { dock: DockPosition; expanded: boolean };
  type SetupResult = { autostartConfigured: boolean };
  type SystemMetrics = {
    cpuUsage: number;
    cpuTemperature: number | null;
    gameFps: number | null;
  };

  const desktopBridgeAvailable = isTauri();
  const emptyMetrics: SystemMetrics = { cpuUsage: 0, cpuTemperature: null, gameFps: null };
  let dockPosition: DockPosition = 'floating';
  let panelOpen = false;
  let setupRunning = false;
  let autostartConfigured = false;
  let metrics = emptyMetrics;
  let notice = '';
  let dragStart: { pointerId: number; x: number; y: number } | null = null;
  let suppressClick = false;

  onMount(() => {
    if (!desktopBridgeAvailable) return;

    let disposed = false;
    let moveTimer: ReturnType<typeof setTimeout> | undefined;
    let unlisteners: (() => void)[] = [];

    void Promise.all([
      listen('orb-setup-request', () => void setPanelOpen(true)),
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
      notice = autostartConfigured ? '已启用登录时启动。' : '登录时启动尚未启用，请重试。';
    } catch (error) {
      notice = `一键设置失败：${String(error)}`;
    } finally {
      setupRunning = false;
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
    try {
      await setPanelOpen(!panelOpen);
    } catch (error) {
      notice = `展开 Orbit 覆盖层失败：${String(error)}`;
    }
  }

  async function handleContextMenu(event: MouseEvent): Promise<void> {
    event.preventDefault();
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
/>

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

      <button
        class="configure-button"
        type="button"
        disabled={setupRunning}
        onclick={beginSetup}
      >
        <span class="button-orbit" aria-hidden="true">O</span>
        <span>{setupRunning ? '正在配置…' : autostartConfigured ? '登录启动已启用' : '一键配置登录启动'}</span>
        <span aria-hidden="true">→</span>
      </button>

      <p class="privacy-note">
        覆盖层始终请求置顶，但独占全屏游戏可能绕过桌面合成层；能否显示取决于系统、游戏全屏模式和显卡驱动。FPS 暂无通用读取接口。
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
