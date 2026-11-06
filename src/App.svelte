<script lang="ts">
  import { invoke, isTauri } from '@tauri-apps/api/core';
  import { canFinishSetup, requiredPermissions, type PermissionState } from './lib/setup';

  type SetupResult = {
    permissions: Record<string, PermissionState>;
    autostartConfigured: boolean;
  };

  const desktopBridgeAvailable = isTauri();
  let permissions: Record<string, PermissionState> = Object.fromEntries(
    requiredPermissions.map(({ id }) => [id, 'unknown']),
  );
  let autostartConfigured = false;
  let setupRunning = false;
  let notice = '';

  $: ready = canFinishSetup(permissions, autostartConfigured);

  async function beginSetup(): Promise<void> {
    if (!desktopBridgeAvailable || setupRunning || ready) return;

    setupRunning = true;
    notice = '';
    try {
      const result = await invoke<SetupResult>('complete_first_run_setup');
      permissions = result.permissions;
      autostartConfigured = result.autostartConfigured;
      if (!canFinishSetup(permissions, autostartConfigured)) {
        notice = '仍有必需设置未完成，请按系统提示重试或前往系统设置。';
      }
    } catch {
      notice = '原生授权流程尚未接入；没有任何权限或启动项被标记为已完成。';
    } finally {
      setupRunning = false;
    }
  }
</script>

<svelte:head>
  <title>首次设置 · Orbit</title>
  <meta name="description" content="配置 Orbit 所需的系统权限和登录启动项" />
</svelte:head>

<div class="app-shell">
  <main class="main-panel">
    <header class="topbar">
      <a class="brand" href="/" aria-label="Orbit 首页">
        <span class="brand-mark" aria-hidden="true">O</span>
        <span>ORBIT</span>
      </a>
      <span class:connected={desktopBridgeAvailable} class="environment-tag">
        <span class="environment-dot"></span>
        {desktopBridgeAvailable ? '桌面环境' : '浏览器预览'}
      </span>
    </header>

    <section class="setup-content">
      <div class="intro">
        <p class="kicker">首次启动</p>
        <h1>让 Orbit 准备就绪<span class="title-period">。</span></h1>
        <p class="intro-copy">
          一次点击，开始配置 Orbit 所需权限并启用登录时启动。系统可能会逐项显示确认窗口。
        </p>
      </div>

      <div class="section-heading">
        <div>
          <h2>将申请和设置</h2>
          <p>系统会根据平台逐项确认，Orbit 不会代替你点击系统授权。</p>
        </div>
        <span class="required-label">必需</span>
      </div>

      <div class="permission-list">
        {#each requiredPermissions as permission}
          <div class="permission-row">
            <span class:granted={permissions[permission.id] === 'granted'} class="permission-mark">
              {permissions[permission.id] === 'granted' ? '✓' : ''}
            </span>
            <div class="permission-copy">
              <h3>{permission.title}</h3>
              <p>{permission.purpose}</p>
            </div>
            <span class="status-label">
              {permissions[permission.id] === 'granted' ? '已完成' : '待申请'}
            </span>
          </div>
        {/each}
        <div class="permission-row">
          <span class:granted={autostartConfigured} class="permission-mark">
            {autostartConfigured ? '✓' : ''}
          </span>
          <div class="permission-copy">
            <h3>登录时启动</h3>
            <p>完成首次设置后可在设置中关闭。</p>
          </div>
          <span class="status-label">{autostartConfigured ? '已完成' : '待设置'}</span>
        </div>
      </div>

      <p class="consent-note">
        点击后 Orbit 会依次发起权限申请和启动项注册。部分系统权限必须在系统弹窗或设置页中由你确认。
      </p>

      {#if !desktopBridgeAvailable}
        <p class="notice" role="status">
          浏览器预览仅展示流程；桌面端授权和登录启动桥接尚未接入。
        </p>
      {/if}

      {#if notice}
        <p class="notice" role="status">{notice}</p>
      {/if}

      <footer class="setup-footer">
        <span class="gate-note">
          {ready ? '必需设置已完成' : '完成必需设置后才能进入 Orbit'}
        </span>
        <button
          class="continue-button"
          type="button"
          disabled={!desktopBridgeAvailable || setupRunning || ready}
          onclick={beginSetup}
        >
          {setupRunning ? '正在申请…' : ready ? '配置完成' : '同意并开始配置'}
          <span aria-hidden="true">{setupRunning ? '…' : '→'}</span>
        </button>
      </footer>
    </section>
  </main>
</div>
