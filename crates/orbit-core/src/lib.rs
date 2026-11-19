mod audit;
mod bindings;
mod contract;
mod error;
mod mcp;

pub use audit::AuditLog;
pub use mcp::{
    BridgeInfo, McpCallRequest, McpTool, is_exposed, normalize_arguments, read_only_tools,
};
pub use bindings::{
    BINDINGS_VERSION, Binding, BindingAction, BindingEvent, BindingsConfig, MAX_BINDINGS,
    MAX_SHORTCUTS, MENUS, Modifier, ShortcutBinding,
};
pub use error::{ErrorKind, OrbitError};
pub use contract::{
    ClipFlagParams, ClipIdParams, ClipItem, ClipListParams, ClipMonitorParams, ClipWriteParams,
    ClipboardChanged, CommandCategory, CommandRequest, CommandResponse, CommandSpec,
    CommandStatus, ConvertFormatParams, DashInfo, DockPosition, EmptyParams, EventBindingKeyParams,
    EventSpec, KbEvent, KbReplayParams, KbReplayResult, RecordingItem, CaptureRegion,
    InputMouseParams, MouseAction, KbRecordStateEvent,
    MonitorSnapshot, OcrImageParams, OcrLine, OcrResult, OcrWord,
    OverlaySnapshot, PermissionState, RecordStartParams, RecordStateEvent, RecoveryItem,
    RegionFinishParams, RegionSelectionStart, RemoveShortcutParams, RemoteEndpoint,
    RemoteRequestParams, RemoteScreenshotResult, RemoteStartParams, RemoteStatus, RiskLevel,
    ScreenshotMode, ScreenshotRequest, SetAutostartParams, SetElevatedParams, SetOverlayOpenParams,
    SetBallRadiusParams, SetupResult, SystemInfo, UpsertEventBindingParams, UpsertShortcutParams,
    WindowLayout, WindowSize, command_catalog, event_catalog,
};
