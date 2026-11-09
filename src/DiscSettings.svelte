<script lang="ts">
  export let autostartConfigured = false;
  export let setupRunning = false;
  export let notice = '';
  export let layoutCount = 0;
  export let onedit: () => void = () => {};
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

    {#if notice}<p class="settings-notice" role="status">{notice}</p>{/if}
    <p class="settings-tip">调整页中拖动扇区排序，拖到中心或拖出圆盘移除，拖动顶部两个圆点调整内圆与外圈大小，点击中心球加回功能。</p>
  </div>

  <div class="settings-actions">
    <button class="settings-btn quit" type="button" onclick={onquit}>退出 Orbit</button>
    <button class="settings-btn primary" type="button" onclick={onclose}>返回</button>
  </div>
</div>
