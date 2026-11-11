import { invoke } from '@tauri-apps/api/core';
import type { CommandResponse } from '../crates/orbit-core/bindings/CommandResponse';

/**
 * 命令总线的唯一前端入口：所有 UI 调用都走 execute_command。
 * 旧直连命令（如 record_start / clip_list）已删除，避免两套入口行为漂移。
 */
export async function runCommand<T>(
  commandId: string,
  params: Record<string, unknown> = {},
): Promise<T> {
  const response = await invoke<CommandResponse>('execute_command', {
    request: { commandId, params },
  });
  if (response.status !== 'ok') {
    throw new Error(response.error ?? `命令 ${commandId} 执行失败。`);
  }
  return response.data as T;
}
