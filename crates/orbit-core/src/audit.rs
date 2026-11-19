use rusqlite::{Connection, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::CommandStatus;

#[derive(Clone)]
pub struct AuditLog(Arc<Mutex<Connection>>);

impl AuditLog {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> rusqlite::Result<Self> {
        let version: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < 1 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE audit_log (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     ts INTEGER NOT NULL,
                     command_id TEXT NOT NULL,
                     source TEXT NOT NULL,
                     params_hash TEXT NOT NULL,
                     result TEXT NOT NULL,
                     duration_ms INTEGER
                 );
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    pub fn record(
        &self,
        command_id: &str,
        params: &Value,
        status: CommandStatus,
        duration_ms: u64,
        source: &str,
    ) -> Result<(), String> {
        let serialized = serde_json::to_vec(params).map_err(|error| error.to_string())?;
        let params_hash = format!("{:x}", Sha256::digest(serialized));
        let result = match status {
            CommandStatus::Ok => "ok",
            CommandStatus::Error => "err",
        };
        self.insert(command_id, &params_hash, result, duration_ms, source)
    }

    /// 被拒绝的调用（MCP 白名单拦截等）：result = `denied`，duration 记 0。
    pub fn record_denied(&self, command_id: &str, params: &Value, source: &str) -> Result<(), String> {
        let serialized = serde_json::to_vec(params).map_err(|error| error.to_string())?;
        let params_hash = format!("{:x}", Sha256::digest(serialized));
        self.insert(command_id, &params_hash, "denied", 0, source)
    }

    fn insert(
        &self,
        command_id: &str,
        params_hash: &str,
        result: &str,
        duration_ms: u64,
        source: &str,
    ) -> Result<(), String> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis() as i64;
        let connection = self.0.lock().map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT INTO audit_log
                (ts, command_id, source, params_hash, result, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    timestamp,
                    command_id,
                    source,
                    params_hash,
                    result,
                    duration_ms.min(i64::MAX as u64) as i64
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_log_records_command_outcomes_with_source() {
        let audit = AuditLog::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        audit
            .record(
                "screenshot.capture",
                &serde_json::json!({"mode": "fullscreen"}),
                CommandStatus::Error,
                12,
                "orb",
            )
            .unwrap();
        audit
            .record_denied("clipboard.clear", &serde_json::json!({}), "mcp")
            .unwrap();

        let connection = audit.0.lock().unwrap();
        let (command_id, result, duration_ms, source): (String, String, i64, String) = connection
            .query_row(
                "SELECT command_id, result, duration_ms, source FROM audit_log ORDER BY id LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(command_id, "screenshot.capture");
        assert_eq!(result, "err");
        assert_eq!(duration_ms, 12);
        assert_eq!(source, "orb");

        let (denied_id, denied_result, denied_source): (String, String, String) = connection
            .query_row(
                "SELECT command_id, result, source FROM audit_log WHERE result = 'denied'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(denied_id, "clipboard.clear");
        assert_eq!(denied_result, "denied");
        assert_eq!(denied_source, "mcp");
    }
}
