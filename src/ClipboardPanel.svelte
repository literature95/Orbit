<script lang="ts">
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import type { ClipItem } from '../crates/orbit-core/bindings/ClipItem';
  import type { ClipboardChanged } from '../crates/orbit-core/bindings/ClipboardChanged';
  import { runCommand } from './bus';

  type Filter = 'all' | 'favorite' | 'pinned' | 'text' | 'files' | 'image';

  const FILTERS: { id: Filter; label: string }[] = [
    { id: 'all', label: '全部' },
    { id: 'favorite', label: '收藏' },
    { id: 'pinned', label: '置顶' },
    { id: 'text', label: '文本' },
    { id: 'files', label: '文件' },
    { id: 'image', label: '图片' },
  ];

  let items: ClipItem[] = [];
  let query = '';
  let filter: Filter = 'all';
  let monitoring = true;
  let status = '';
  let clearArmed = false;
  // 首帧占位；真实快捷键由 clipboard.shortcut 命令在 onMount 时下发。
  let shortcutLabel = '快捷键';
  let previewItem: ClipItem | null = null;
  let previewUrl = '';
  let searchInput: HTMLInputElement | null = null;
  let statusTimer: ReturnType<typeof setTimeout> | undefined;
  let clearTimer: ReturnType<typeof setTimeout> | undefined;

  function setStatus(message: string): void {
    status = message;
    if (statusTimer) clearTimeout(statusTimer);
    statusTimer = setTimeout(() => { status = ''; }, 2600);
  }

  async function refresh(): Promise<void> {
    try {
      items = await runCommand<ClipItem[]>('clipboard.list', {
        query: query.trim() || null,
        filter,
        limit: 200,
      });
      if (previewItem && !items.some((entry) => entry.id === previewItem?.id)) {
        closePreview();
      }
    } catch (error) {
      setStatus(`读取失败：${String(error)}`);
    }
  }

  async function toggleMonitor(): Promise<void> {
    try {
      monitoring = await runCommand<boolean>('clipboard.set_monitor', { enabled: !monitoring });
      setStatus(monitoring ? '监听已恢复' : '监听已暂停');
    } catch (error) {
      setStatus(`切换失败：${String(error)}`);
    }
  }

  async function writeBack(item: ClipItem): Promise<void> {
    try {
      await runCommand('clipboard.write_back', { id: item.id });
      await refresh();
      await closeWindow();
    } catch (error) {
      setStatus(`写回失败：${String(error)}`);
    }
  }

  async function toggleFlag(item: ClipItem, field: 'pinned' | 'favorite'): Promise<void> {
    const next = !item[field];
    try {
      await runCommand(field === 'pinned' ? 'clipboard.set_pin' : 'clipboard.set_favorite', {
        id: item.id,
        value: next,
      });
      await refresh();
    } catch (error) {
      setStatus(`操作失败：${String(error)}`);
    }
  }

  async function removeItem(item: ClipItem): Promise<void> {
    try {
      await runCommand('clipboard.delete', { id: item.id });
      await refresh();
    } catch (error) {
      setStatus(`删除失败：${String(error)}`);
    }
  }

  async function clearHistory(): Promise<void> {
    if (!clearArmed) {
      clearArmed = true;
      setStatus('再点一次确认清空（置顶/收藏会保留）');
      clearTimer = setTimeout(() => { clearArmed = false; }, 3000);
      return;
    }
    clearArmed = false;
    if (clearTimer) clearTimeout(clearTimer);
    try {
      await runCommand('clipboard.clear');
      await refresh();
      setStatus('已清空普通条目');
    } catch (error) {
      setStatus(`清空失败：${String(error)}`);
    }
  }

  async function openPreview(item: ClipItem): Promise<void> {
    if (previewItem?.id === item.id) {
      closePreview();
      return;
    }
    previewItem = item;
    previewUrl = '';
    try {
      previewUrl = await runCommand<string>('clipboard.image_preview', { id: item.id });
    } catch (error) {
      previewUrl = '';
      setStatus(`预览失败：${String(error)}`);
    }
  }

  function closePreview(): void {
    previewItem = null;
    previewUrl = '';
  }

  async function closeWindow(): Promise<void> {
    try { await runCommand('clipboard.close_window'); } catch { /* 忽略 */ }
  }

  function rowKey(event: KeyboardEvent, item: ClipItem): void {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      void writeBack(item);
    }
  }

  function handleKeyDown(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
      event.preventDefault();
      if (previewItem) {
        closePreview();
        return;
      }
      void closeWindow();
    }
  }

  function formatTime(ms: number): string {
    const diff = Date.now() - ms;
    if (diff < 60_000) return '刚刚';
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
    const d = new Date(ms);
    const pad = (v: number) => String(v).padStart(2, '0');
    return `${d.getMonth() + 1}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
  }

  function selectFilter(next: Filter): void {
    filter = next;
    void refresh();
  }

  onMount(() => {
    void refresh();
    void runCommand<boolean>('clipboard.get_monitor')
      .then((value) => { monitoring = value; })
      .catch(() => { /* 忽略 */ });
    void runCommand<string>('clipboard.shortcut')
      .then((value) => { shortcutLabel = value; })
      .catch(() => { /* 忽略 */ });
    searchInput?.focus();

    let disposed = false;
    const disposers: (() => void)[] = [];
    const attach = (registration: Promise<() => void>): void => {
      void registration
        .then((dispose) => { if (disposed) dispose(); else disposers.push(dispose); })
        .catch(() => { /* 忽略 */ });
    };
    attach(listen<ClipboardChanged>('clip://changed', () => { void refresh(); }));
    // 全局快捷键 / 圆盘唤起窗口时，重置预览并聚焦搜索框
    attach(listen('clip://shown', () => {
      closePreview();
      void refresh();
      searchInput?.focus();
    }));

    // 仅在相对时间跨分钟时刷新显示，不做高频轮询
    const timer = setInterval(() => { void refresh(); }, 30_000);
    return () => {
      disposed = true;
      disposers.forEach((dispose) => dispose());
      clearInterval(timer);
      if (statusTimer) clearTimeout(statusTimer);
      if (clearTimer) clearTimeout(clearTimer);
    };
  });
</script>

<svelte:window onkeydown={handleKeyDown} />

<div class="clip-shell">
  <header class="clip-header" data-tauri-drag-region>
    <div class="clip-title">
      <span class="clip-logo">📋</span>
      <span>剪贴板</span>
      <span class="clip-total">{items.length}</span>
    </div>
    <button class="clip-icon-btn" type="button" title="关闭" aria-label="关闭" onclick={() => void closeWindow()}>✕</button>
  </header>

  <div class="clip-tools">
    <input
      class="clip-search"
      type="text"
      placeholder="搜索内容…"
      bind:this={searchInput}
      bind:value={query}
      oninput={() => void refresh()}
      aria-label="搜索剪贴板历史"
    />
    <button
      class="clip-icon-btn"
      class:off={!monitoring}
      type="button"
      title={monitoring ? '暂停监听' : '恢复监听'}
      aria-label={monitoring ? '暂停监听' : '恢复监听'}
      onclick={() => void toggleMonitor()}
    >{monitoring ? '⏸' : '▶'}</button>
  </div>

  <nav class="clip-filters" aria-label="筛选">
    {#each FILTERS as f}
      <button
        class="clip-filter"
        class:active={filter === f.id}
        type="button"
        onclick={() => selectFilter(f.id)}
      >{f.label}</button>
    {/each}
  </nav>

  {#if previewItem}
    <div class="clip-preview-pane">
      <div class="clip-preview-head">
        <span>🖼 图片预览</span>
        <span class="clip-preview-size">{previewItem.preview.replace('🖼 ', '')}</span>
        <button class="clip-icon-btn" type="button" title="返回列表" aria-label="返回列表" onclick={closePreview}>✕</button>
      </div>
      <div class="clip-preview-body">
        {#if previewUrl}
          <img src={previewUrl} alt="剪贴板图片预览" />
        {:else}
          <span class="clip-preview-loading">加载中…</span>
        {/if}
      </div>
      <button
        class="clip-preview-use"
        type="button"
        onclick={() => { if (previewItem) void writeBack(previewItem); }}
      >写回剪贴板并关闭</button>
    </div>
  {:else}
  <div class="clip-list">
    {#if items.length === 0}
      <div class="clip-empty">
        <div class="clip-empty-icon">🗂</div>
        <p>暂无记录</p>
        <small>{query ? '换个关键词试试' : monitoring ? '复制任意文本或文件即可自动收录' : '监听已暂停'}</small>
      </div>
    {:else}
      {#each items as item (item.id)}
        <div
          class="clip-row"
          role="button"
          tabindex="0"
          onclick={() => void writeBack(item)}
          onkeydown={(event) => rowKey(event, item)}
        >
          <span class="clip-kind">{item.kind === 'files' ? '📁' : item.kind === 'image' ? '🖼' : '📄'}</span>
          <div class="clip-main">
            <div class="clip-preview">{item.preview}</div>
            <div class="clip-meta">
              <span>{formatTime(item.updatedAt)}</span>
              <span>· {item.useCount} 次</span>
              {#if item.pinned}<span class="clip-badge">置顶</span>{/if}
              {#if item.favorite}<span class="clip-badge star">收藏</span>{/if}
            </div>
          </div>
          <div class="clip-actions">
            {#if item.kind === 'image'}
              <button
                class="clip-icon-btn small"
                type="button"
                title="预览图片"
                aria-label="预览图片"
                onclick={(event) => { event.stopPropagation(); void openPreview(item); }}
              >👁</button>
            {/if}
            <button
              class="clip-icon-btn small"
              class:on={item.pinned}
              type="button"
              title={item.pinned ? '取消置顶' : '置顶'}
              aria-label={item.pinned ? '取消置顶' : '置顶'}
              onclick={(event) => { event.stopPropagation(); void toggleFlag(item, 'pinned'); }}
            >📌</button>
            <button
              class="clip-icon-btn small"
              class:on={item.favorite}
              type="button"
              title={item.favorite ? '取消收藏' : '收藏'}
              aria-label={item.favorite ? '取消收藏' : '收藏'}
              onclick={(event) => { event.stopPropagation(); void toggleFlag(item, 'favorite'); }}
            >⭐</button>
            <button
              class="clip-icon-btn small"
              type="button"
              title="删除"
              aria-label="删除"
              onclick={(event) => { event.stopPropagation(); void removeItem(item); }}
            >🗑</button>
          </div>
        </div>
      {/each}
    {/if}
  </div>
  {/if}

  <footer class="clip-footer">
    <span class="clip-status">{status || (monitoring ? `正在监听 · ${shortcutLabel} 唤起` : '监听已暂停')}</span>
    <button class="clip-clear" class:armed={clearArmed} type="button" onclick={() => void clearHistory()}>
      {clearArmed ? '确认清空？' : '清空'}
    </button>
  </footer>
</div>

<style>
  .clip-shell {
    position: absolute;
    top: var(--orb-panel-inset);
    left: var(--orb-panel-inset);
    width: calc(var(--orb-clipboard-w) - var(--orb-panel-inset) - var(--orb-panel-inset));
    height: calc(var(--orb-clipboard-h) - var(--orb-panel-inset) - var(--orb-panel-inset));
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    border: 1px solid var(--panel-border);
    border-radius: 16px;
    background: var(--panel-bg);
    backdrop-filter: blur(18px);
    -webkit-backdrop-filter: blur(18px);
    box-shadow: 0 18px 50px rgb(0 0 0 / 55%);
    color: var(--text-main);
    font-family: 'Segoe UI', 'Noto Sans CJK SC', 'Microsoft YaHei', sans-serif;
    font-size: 12px;
    overflow: hidden;
    user-select: none;
  }
  .clip-shell * { box-sizing: border-box; }

  .clip-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px 10px;
    border-bottom: 1px solid var(--panel-border);
    flex: 0 0 auto;
  }
  .clip-title { display: flex; align-items: center; gap: 8px; font-size: 13px; font-weight: 600; letter-spacing: .4px; }
  .clip-logo { font-size: 15px; }
  .clip-total {
    padding: 1px 7px;
    border-radius: 999px;
    background: rgba(120, 150, 255, .16);
    color: var(--text-label);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
  }

  .clip-icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    padding: 0;
    border: 1px solid transparent;
    border-radius: 8px;
    background: transparent;
    color: var(--text-dim);
    font-size: 13px;
    cursor: pointer;
    transition: background .18s ease, color .18s ease;
  }
  .clip-icon-btn:hover { background: var(--panel-hover); color: var(--text-main); }
  .clip-icon-btn.off { color: var(--text-label); opacity: .75; }
  .clip-icon-btn.small { width: 24px; height: 24px; font-size: 11px; opacity: .55; }
  .clip-icon-btn.small.on { opacity: 1; }
  .clip-row:hover .clip-icon-btn.small { opacity: 1; }

  .clip-tools { display: flex; gap: 8px; padding: 10px 12px 8px; flex: 0 0 auto; }
  .clip-search {
    flex: 1;
    min-width: 0;
    height: 30px;
    padding: 0 10px;
    border: 1px solid var(--panel-border);
    border-radius: 9px;
    background: rgba(10, 14, 26, .55);
    color: var(--text-main);
    font-size: 12px;
    outline: none;
    user-select: text;
  }
  .clip-search::placeholder { color: var(--text-dim); opacity: .7; }
  .clip-search:focus { border-color: rgba(150, 180, 255, .5); }

  .clip-filters { display: flex; gap: 4px; padding: 0 12px 8px; flex: 0 0 auto; }
  .clip-filter {
    flex: 1;
    padding: 5px 0;
    border: 1px solid transparent;
    border-radius: 7px;
    background: rgba(120, 150, 255, .07);
    color: var(--text-dim);
    font-size: 11px;
    cursor: pointer;
  }
  .clip-filter:hover { background: var(--panel-hover); color: var(--text-main); }
  .clip-filter.active { background: rgba(120, 150, 255, .22); color: var(--text-main); border-color: var(--panel-border); }

  .clip-list { flex: 1 1 auto; overflow-y: auto; padding: 2px 8px 6px; }
  .clip-list::-webkit-scrollbar { width: 6px; }
  .clip-list::-webkit-scrollbar-thumb { border-radius: 3px; background: rgba(130, 160, 255, .22); }

  .clip-preview-pane {
    flex: 1 1 auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 0;
    margin: 2px 8px 6px;
    padding: 10px;
    border: 1px solid var(--panel-border);
    border-radius: 12px;
    background: rgba(6, 9, 18, .5);
  }
  .clip-preview-head { display: flex; align-items: center; gap: 8px; color: var(--text-dim); font-size: 11px; }
  .clip-preview-head > span:first-child { color: var(--text-main); font-weight: 600; }
  .clip-preview-size { flex: 1 1 auto; font-variant-numeric: tabular-nums; }
  .clip-preview-body {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 9px;
    background: rgba(0, 0, 0, .35);
    overflow: hidden;
  }
  .clip-preview-body img { max-width: 100%; max-height: 100%; object-fit: contain; }
  .clip-preview-loading { color: var(--text-dim); font-size: 11px; }
  .clip-preview-use {
    flex: 0 0 auto;
    padding: 7px 0;
    border: 1px solid var(--panel-border);
    border-radius: 9px;
    background: rgba(120, 150, 255, .16);
    color: var(--text-main);
    font-size: 11px;
    cursor: pointer;
  }
  .clip-preview-use:hover { background: var(--panel-hover); }

  .clip-row {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 8px 8px;
    margin-bottom: 3px;
    border: 1px solid transparent;
    border-radius: 10px;
    background: rgba(120, 150, 255, .05);
    cursor: pointer;
    transition: background .16s ease, border-color .16s ease;
  }
  .clip-row:hover { background: var(--panel-hover); border-color: var(--panel-border); }
  .clip-row:focus-visible { outline: 1px solid var(--text-label); }
  .clip-kind { flex: 0 0 auto; font-size: 15px; }
  .clip-main { flex: 1 1 auto; min-width: 0; }
  .clip-preview {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-main);
  }
  .clip-meta { display: flex; gap: 5px; color: var(--text-dim); font-size: 10px; opacity: .85; }
  .clip-badge { color: var(--text-label); }
  .clip-badge.star { color: #f7c66b; }
  .clip-actions { display: flex; gap: 1px; flex: 0 0 auto; }

  .clip-empty { padding: 60px 20px; text-align: center; color: var(--text-dim); }
  .clip-empty-icon { font-size: 30px; opacity: .6; }
  .clip-empty p { margin: 12px 0 4px; color: var(--text-main); font-size: 13px; }
  .clip-empty small { font-size: 11px; opacity: .8; }

  .clip-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 12px;
    border-top: 1px solid var(--panel-border);
    flex: 0 0 auto;
    color: var(--text-dim);
    font-size: 10px;
  }
  .clip-status { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .clip-clear {
    flex: 0 0 auto;
    padding: 4px 9px;
    border: 1px solid var(--panel-border);
    border-radius: 7px;
    background: transparent;
    color: var(--text-dim);
    font-size: 10px;
    cursor: pointer;
  }
  .clip-clear:hover { background: var(--panel-hover); color: var(--text-main); }
  .clip-clear.armed { color: #fda4af; border-color: rgba(253, 164, 175, .5); }
</style>
