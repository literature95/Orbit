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
    ) -> Result<(), String> {
        let serialized = serde_json::to_vec(params).map_err(|error| error.to_string())?;
        let params_hash = format!("{:x}", Sha256::digest(serialized));
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis() as i64;
        let result = match status {
            CommandStatus::Ok => "ok",
            CommandStatus::Error => "err",
        };
        let connection = self.0.lock().map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT INTO audit_log
                (ts, command_id, source, params_hash, result, duration_ms)
             VALUES (?1, ?2, 'orb', ?3, ?4, ?5)",
                params![
                    timestamp,
                    command_id,
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
    fn audit_log_records_command_outcomes() {
        let audit = AuditLog::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        audit
            .record(
                "screenshot.capture",
                &serde_json::json!({"mode": "fullscreen"}),
                CommandStatus::Error,
                12,
            )
            .unwrap();

        let connection = audit.0.lock().unwrap();
        let (command_id, result, duration_ms): (String, String, i64) = connection
            .query_row(
                "SELECT command_id, result, duration_ms FROM audit_log",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(command_id, "screenshot.capture");
        assert_eq!(result, "err");
        assert_eq!(duration_ms, 12);
    }
}
