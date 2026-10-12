//! Account-local download history. Only lifecycle transitions write SQLite.
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use super::TaskSnapshot;
use crate::{account::AccountContext, AppState};

const RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const RECORD_LIMIT: i64 = 500;
pub(crate) const CHANGED_EVENT: &str = "cliproam://download-records-changed";

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadRecord {
    pub id: String,
    pub entry_id: String,
    pub label: String,
    pub size: u64,
    pub file_count: usize,
    pub status: String,
    pub created_ms: i64,
    pub finished_ms: Option<i64>,
    pub received_bytes: u64,
    pub destination: String,
    pub error: Option<String>,
    pub files: Vec<TaskSnapshot>,
}

pub(crate) fn initialize(connection: &Connection) -> Result<(), String> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS download_records (
            id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            created_ms INTEGER NOT NULL,
            finished_ms INTEGER,
            data TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS download_records_created ON download_records(created_ms DESC);",
    ).map_err(|error| error.to_string())?;
    // Called once per pooled database on process startup, never on account switches.
    let mut records = list(connection)?;
    let now = chrono::Utc::now().timestamp_millis();
    for record in records.iter_mut().filter(|record| record.finished_ms.is_none()) {
        record.status = "interrupted".into();
        record.finished_ms = Some(now);
        record.error = Some("应用退出，下载未完成".into());
        write(connection, record)?;
    }
    trim(connection, now)
}

fn write(connection: &Connection, record: &DownloadRecord) -> Result<(), String> {
    let data = serde_json::to_string(record).map_err(|error| error.to_string())?;
    connection.execute(
        "INSERT INTO download_records(id,status,created_ms,finished_ms,data) VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(id) DO UPDATE SET status=excluded.status,finished_ms=excluded.finished_ms,data=excluded.data",
        params![record.id, record.status, record.created_ms, record.finished_ms, data],
    ).map_err(|error| error.to_string())?;
    Ok(())
}

fn trim(connection: &Connection, now: i64) -> Result<(), String> {
    connection.execute("DELETE FROM download_records WHERE finished_ms IS NOT NULL AND finished_ms < ?1", [now - RETENTION_MS])
        .map_err(|error| error.to_string())?;
    // Active downloads are never evicted, even when more than 500 are queued.
    connection.execute(
        "DELETE FROM download_records WHERE id IN (
            SELECT id FROM download_records WHERE finished_ms IS NOT NULL
            ORDER BY created_ms DESC, id DESC LIMIT -1 OFFSET ?1)",
        [RECORD_LIMIT],
    ).map_err(|error| error.to_string())?;
    Ok(())
}

fn list(connection: &Connection) -> Result<Vec<DownloadRecord>, String> {
    let mut statement = connection.prepare("SELECT data FROM download_records ORDER BY created_ms DESC, id DESC")
        .map_err(|error| error.to_string())?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(|error| error.to_string())?;
    rows.map(|row| serde_json::from_str(&row.map_err(|error| error.to_string())?).map_err(|error| error.to_string())).collect()
}

fn load(connection: &Connection, id: &str) -> Result<Option<DownloadRecord>, String> {
    let data: Option<String> = connection.query_row("SELECT data FROM download_records WHERE id=?1", [id], |row| row.get(0))
        .optional().map_err(|error| error.to_string())?;
    data.map(|data| serde_json::from_str(&data).map_err(|error| error.to_string())).transpose()
}

pub(crate) fn notify(app: &AppHandle, account: &AccountContext) {
    let _ = app.emit(CHANGED_EVENT, &account.key);
}

pub(crate) fn begin(app: &AppHandle, account: &AccountContext, record: &DownloadRecord) -> Result<(), String> {
    account.with_database(|connection| {
        write(connection, record)?;
        trim(connection, record.created_ms)
    })?;
    notify(app, account);
    Ok(())
}

pub(crate) fn finish(
    app: &AppHandle, account: &AccountContext, id: &str, status: &str,
    error: Option<String>, files: Option<Vec<TaskSnapshot>>,
) -> Result<(), String> {
    account.with_database(|connection| finish_record(connection, id, status, error, files))?;
    notify(app, account);
    Ok(())
}

fn finish_record(
    connection: &Connection, id: &str, status: &str,
    error: Option<String>, files: Option<Vec<TaskSnapshot>>,
) -> Result<(), String> {
    let Some(mut record) = load(connection, id)? else { return Ok(()) };
    // Export cancellation must not overwrite an earlier transfer failure.
    if record.finished_ms.is_some() { return Ok(()) }
    record.status = status.into();
    record.error = error;
    if let Some(files) = files {
        record.received_bytes = files.iter().map(|file| file.received_bytes).sum();
        record.files = files;
    }
    if status == "succeeded" || status == "saving" { record.received_bytes = record.size; }
    if status != "saving" { record.finished_ms = Some(chrono::Utc::now().timestamp_millis()); }
    write(connection, &record)?;
    trim(connection, chrono::Utc::now().timestamp_millis())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn download_records(state: State<'_, AppState>, session_id: String) -> Result<Vec<DownloadRecord>, String> {
    let account = state.account(&session_id)?;
    account.with_database(|connection| {
        trim(connection, chrono::Utc::now().timestamp_millis())?;
        list(connection)
    })
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn remove_download_records(
    app: AppHandle, state: State<'_, AppState>, session_id: String, record_id: Option<String>,
) -> Result<(), String> {
    let account = state.account(&session_id)?;
    account.with_database(|connection| {
        connection.execute("DELETE FROM download_records WHERE finished_ms IS NOT NULL AND (?1 IS NULL OR id=?1)", [record_id])
            .map_err(|error| error.to_string())?;
        Ok(())
    })?;
    notify(&app, &account);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(id: &str, created_ms: i64, finished: bool) -> DownloadRecord {
        DownloadRecord {
            id: id.into(), entry_id: "entry".into(), label: "folder".into(), size: 100,
            file_count: 3, status: if finished { "succeeded" } else { "downloading" }.into(),
            created_ms, finished_ms: finished.then_some(created_ms), received_bytes: 0,
            destination: "应用缓存".into(), error: None, files: vec![],
        }
    }
    #[test]
    fn restart_marks_active_records_interrupted_and_keeps_results() {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        write(&connection, &record("active", now, false)).unwrap();
        write(&connection, &record("done", now, true)).unwrap();
        initialize(&connection).unwrap();
        assert_eq!(load(&connection, "active").unwrap().unwrap().status, "interrupted");
        assert_eq!(load(&connection, "done").unwrap().unwrap().status, "succeeded");
    }
    #[test]
    fn retention_and_limit_never_remove_active_records() {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        write(&connection, &record("active", now - RETENTION_MS * 2, false)).unwrap();
        write(&connection, &record("old", now - RETENTION_MS - 1, true)).unwrap();
        for index in 0..510 { write(&connection, &record(&format!("done-{index}"), now + index, true)).unwrap(); }
        trim(&connection, now).unwrap();
        assert!(load(&connection, "active").unwrap().is_some());
        assert!(load(&connection, "old").unwrap().is_none());
        assert_eq!(list(&connection).unwrap().len(), 501);
        connection.execute("DELETE FROM download_records WHERE finished_ms IS NOT NULL", []).unwrap();
        assert_eq!(list(&connection).unwrap().len(), 1);
    }
    #[test]
    fn export_result_is_final_and_cleanup_cannot_overwrite_failure() {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        write(&connection, &record("export", now, false)).unwrap();
        finish_record(&connection, "export", "saving", None, Some(vec![])).unwrap();
        let saving = load(&connection, "export").unwrap().unwrap();
        assert!(saving.finished_ms.is_none());
        assert_eq!(saving.received_bytes, 100);
        finish_record(&connection, "export", "failed", Some("目标目录不可写".into()), None).unwrap();
        finish_record(&connection, "export", "cancelled", None, None).unwrap();
        let failed = load(&connection, "export").unwrap().unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.error.as_deref(), Some("目标目录不可写"));
        assert!(failed.finished_ms.is_some());
    }
    #[test]
    fn disk_history_survives_reopen_and_other_accounts_have_no_records() {
        let path = std::env::temp_dir().join(format!("cliproam-download-records-{}.sqlite", uuid::Uuid::new_v4()));
        {
            let connection = Connection::open(&path).unwrap();
            initialize(&connection).unwrap();
            let now = chrono::Utc::now().timestamp_millis();
            let mut downloaded = record("download", now, true);
            downloaded.destination = "D:/Downloads/folder".into();
            write(&connection, &downloaded).unwrap();
        }
        {
            let connection = Connection::open(&path).unwrap();
            initialize(&connection).unwrap();
            let downloaded = load(&connection, "download").unwrap().unwrap();
            assert_eq!(downloaded.file_count, 3);
            assert_eq!(downloaded.destination, "D:/Downloads/folder");
            let other_account = Connection::open_in_memory().unwrap();
            initialize(&other_account).unwrap();
            assert!(list(&other_account).unwrap().is_empty());
        }
        std::fs::remove_file(path).unwrap();
    }

}
