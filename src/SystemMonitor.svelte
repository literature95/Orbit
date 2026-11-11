<script lang="ts">
  import { isTauri } from '@tauri-apps/api/core';
  import { onMount, onDestroy } from 'svelte';
  import type { MonitorSnapshot } from '../crates/orbit-core/bindings/MonitorSnapshot';
  import { runCommand } from './bus';

  export let onclose: () => void = () => {};

  let snapshot: MonitorSnapshot | null = null;
  let error = '';
  let toggling = false;
  let timer: ReturnType<typeof setInterval> | null = null;

  function formatUptime(secs: number): string {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    if (h > 0) return `${h} 小时 ${m} 分`;
    return `${m} 分`;
  }

  function formatGb(mb: number): string {
    return (mb / 1024).toFixed(1);
  }

  function clamp(value: number): number {
    return Math.max(0, Math.min(100, value));
  }

  async function poll(): Promise<void> {
    if (!isTauri()) {
      error = '浏览器预览没有真实系统指标。';
      return;
    }
    try {
      snapshot = await runCommand<MonitorSnapshot>('system.monitor');
      error = '';
    } catch (e) {
      error = `读取系统指标失败：${String(e)}`;
    }
  }

  async function toggleTemperature(): Promise<void> {
    if (!isTauri() || toggling) return;
    toggling = true;
    try {
      await runCommand<boolean>('elevated.set', {
        enabled: !(snapshot?.temperatureEnabled ?? false),
      });
      await poll();
    } catch (e) {
      error = `温度监控切换失败：${String(e)}`;
    } finally {
      toggling = false;
    }
  }

  onMount(() => {
    void poll();
    timer = setInterval(poll, 1000);
  });

  onDestroy(() => {
    if (timer) clearInterval(timer);
  });
</script>

<div class="settings-panel monitor-panel" role="dialog" aria-label="系统监控">
  <div class="settings-head">
    <span class="settings-title">系统监控</span>
    <button class="settings-close" type="button" aria-label="返回圆盘" onclick={onclose}>✕</button>
  </div>

  <div class="settings-body">
    {#if !snapshot}
      <p class="monitor-loading">{error || '正在采样…'}</p>
    {:else}
      <div class="monitor-metric">
        <div class="monitor-metric-head">
          <span>处理器</span>
          <strong>{Math.round(snapshot.cpuPercent)}%</strong>
        </div>
        <div class="monitor-bar"><span style:width="{clamp(snapshot.cpuPercent)}%"></span></div>
      </div>

      <div class="monitor-cores" aria-label="每核占用">
        {#each snapshot.perCore as usage, i}
          <span class="monitor-core" title={`核心 ${i + 1} · ${Math.round(usage)}%`}>
            <i style:height="{Math.max(4, clamp(usage))}%"></i>
          </span>
        {/each}
      </div>

      <div class="monitor-metric">
        <div class="monitor-metric-head">
          <span>内存</span>
          <strong>{Math.round(snapshot.memoryUsedPercent)}%</strong>
        </div>
        <div class="monitor-bar"><span style:width="{clamp(snapshot.memoryUsedPercent)}%"></span></div>
        <div class="monitor-sub">
          {formatGb(snapshot.memoryUsedMb)} / {formatGb(snapshot.memoryTotalMb)} GB
        </div>
      </div>

      <div class="settings-row">
        <span class="settings-row-label">CPU 温度</span>
        {#if snapshot.temperatureC != null}
          <span class="monitor-temp">{Math.round(snapshot.temperatureC)}°C</span>
        {:else}
          <button
            class="settings-toggle"
            class:on={snapshot.temperatureEnabled}
            type="button"
            disabled={toggling}
            title={snapshot.temperatureEnabled ? '已启用，等待 helper 写入温度' : '需要一次 UAC 授权'}
            onclick={toggleTemperature}
          >{snapshot.temperatureEnabled ? '等待数据…' : '启用'}</button>
        {/if}
      </div>

      <div class="settings-row">
        <span class="settings-row-label">运行时长</span>
        <span class="monitor-value">{formatUptime(snapshot.uptimeSecs)}</span>
      </div>

      {#if error}<p class="settings-notice" role="status">{error}</p>{/if}
    {/if}
  </div>

  <div class="settings-actions">
    <span class="monitor-hint">每秒实时采样</span>
    <button class="settings-btn primary" type="button" onclick={onclose}>返回</button>
  </div>
</div>
