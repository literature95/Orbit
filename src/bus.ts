import { invoke } from '@tauri-apps/api/core';
import type { CommandResponse } from '../crates/orbit-core/bindings/CommandResponse';
import type { ErrorKind } from '../crates/orbit-core/bindings/ErrorKind';

/**
 * 命令失败的结构化错误：保留后端 `error_kind`，调用方可按类别分支
 * （参数错误不重试、超时可重试、权限不足应引导用户授权）。
 */
export class CommandError extends Error {
  readonly commandId: string;
  readonly kind: ErrorKind | null;

  constructor(commandId: string, message: string, kind: ErrorKind | null) {
    super(message);
    this.name = 'CommandError';
    this.commandId = commandId;
    this.kind = kind;
  }
}

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
    throw new CommandError(
      commandId,
      response.error ?? `命令 ${commandId} 执行失败。`,
      response.errorKind ?? null,
    );
  }
  return response.data as T;
}
