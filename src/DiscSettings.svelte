<script lang="ts">
  export let autostartConfigured = false;
  export let setupRunning = false;
  export let notice = '';
  export let layoutCount = 0;
  export let recordDir: string | null = null;
  export let replayRunning = false;
  export let mouseBusy = false;
  export let remoteRunning = false;
  export let remoteEndpoint = '';
  export let remoteToken = '';
  export let remoteResult = '';
  export let remoteBusy = false;
  export let remoteEndpointInput = '';
  export let remoteTokenInput = '';
  export let onedit: () => void = () => {};
  export let onpickrecorddir: () => void = () => {};
  export let onclearrecorddir: () => void = () => {};
  export let onreplayrecording: () => void = () => {};
  export let onmousedemo: () => void = () => {};
  export let onremotetoggle: () => void = () => {};
  export let onremotescreenshot: () => void = () => {};
  export let onremoteocr: () => void = () => {};
  export let ontoggleautostart: () => void = () => {};
  export let onquit: () => void = () => {};
  export let onclose: () => void = () => {};
</script>

<div class="settings-panel" role="dialog" aria-label="设置">
  <div class="settings-head">
    <span class="settings-title">设置</span>
    <button class="settings-close" type="button" aria-label="返回圆盘" onclick={onclose}>✕</button>
  </div>

  <div class="settings-body">
    <div class="settings-row">
      <span class="settings-row-label">登录时启动</span>
      <button
        class="settings-toggle"
        class:on={autostartConfigured}
        type="button"
        disabled={setupRunning}
        onclick={ontoggleautostart}
      >{autostartConfigured ? '已开启' : '已关闭'}</button>
    </div>

    <button class="settings-row settings-entry" type="button" onclick={onedit}>
      <span class="settings-row-label">调整轮盘布局</span>
      <span class="settings-entry-value">{layoutCount} 项 ›</span>
    </button>

    <div class="settings-row settings-row-multiline">
      <span class="settings-row-label">录屏保存目录</span>
      <span class="settings-row-value rec-dir" title={recordDir ?? ''}>{recordDir ?? '默认：桌面/Orbit'}</span>
    </div>
    <div class="settings-row rec-dir-actions">
      <button class="settings-btn small" type="button" onclick={onpickrecorddir}>选择</button>
      {#if recordDir}<button class="settings-btn small" type="button" onclick={onclearrecorddir}>清除</button>{/if}
    </div>

    <div class="settings-row">
      <span class="settings-row-label">键鼠回放</span>
      <button
        class="settings-btn small"
        type="button"
        disabled={replayRunning}
        onclick={onreplayrecording}
      >{replayRunning ? '回放中…' : '回放最近一次'}</button>
    </div>

    <div class="settings-row">
      <span class="settings-row-label">鼠标控制演示</span>
      <button
        class="settings-btn small"
        type="button"
        disabled={mouseBusy}
        onclick={onmousedemo}
      >{mouseBusy ? '执行中…' : '移动并单击'}</button>
    </div>

    <div class="settings-row">
      <span class="settings-row-label">远程只读服务</span>
      <button
        class="settings-toggle"
        class:on={remoteRunning}
        type="button"
        onclick={onremotetoggle}
      >{remoteRunning ? '停止' : '启动'}</button>
    </div>
    {#if remoteRunning}
      <div class="settings-row settings-row-multiline remote-meta">
        <span class="settings-row-label">端点</span>
        <span class="settings-row-value remote-token" title={remoteEndpoint}>{remoteEndpoint || '—'}</span>
      </div>
      <div class="settings-row settings-row-multiline remote-meta">
        <span class="settings-row-label">配对令牌</span>
        <span class="settings-row-value remote-token" title={remoteToken}>{remoteToken || '—'}</span>
      </div>
    {/if}

    <div class="settings-row remote-inputs">
      <input
        class="settings-input"
        bind:value={remoteEndpointInput}
        placeholder="对方端点 127.0.0.1:端口"
        aria-label="对方端点"
      />
      <input
        class="settings-input"
        bind:value={remoteTokenInput}
        placeholder="配对令牌"
        aria-label="配对令牌"
      />
    </div>
    <div class="settings-row remote-actions">
      <button class="settings-btn small" type="button" disabled={remoteBusy} onclick={onremotescreenshot}>远程截图</button>
      <button class="settings-btn small" type="button" disabled={remoteBusy} onclick={onremoteocr}>远程 OCR</button>
    </div>
    {#if remoteResult}<p class="settings-notice remote-result" role="status">{remoteResult}</p>{/if}

    {#if notice}<p class="settings-notice" role="status">{notice}</p>{/if}
    <p class="settings-tip">调整页中拖动扇区排序，拖到中心或拖出圆盘移除，拖动顶部两个圆点调整内圆与外圈大小，点击中心球加回功能。</p>
  </div>

  <div class="settings-actions">
    <button class="settings-btn quit" type="button" onclick={onquit}>退出 Orbit</button>
    <button class="settings-btn primary" type="button" onclick={onclose}>返回</button>
  </div>
</div>
