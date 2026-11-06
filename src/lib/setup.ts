export type SetupPermission = {
  id: string;
  title: string;
  purpose: string;
  platformNote: string;
};

export const requiredPermissions: SetupPermission[] = [
  {
    id: 'screen-capture',
    title: '屏幕录制与截图',
    purpose: '用于截图、OCR 与录屏能力。',
    platformNote: '由操作系统的隐私设置单独授权。',
  },
  {
    id: 'accessibility-input',
    title: '辅助功能与输入监听',
    purpose: '用于全局快捷键、键鼠录制与受控回放。',
    platformNote: '权限名称和授权入口因系统而异。',
  },
];

export type PermissionState = 'unknown' | 'granted' | 'denied';

export function canFinishSetup(
  permissions: Record<string, PermissionState>,
  autostartConfigured: boolean,
): boolean {
  return (
    autostartConfigured &&
    requiredPermissions.every((permission) => permissions[permission.id] === 'granted')
  );
}
