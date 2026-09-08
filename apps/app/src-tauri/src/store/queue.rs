use rusqlite::params;
use std::path::Path;

use super::open_history_database;

/// Temporary, pre-publish entry identity. The server assigns the real id on
/// first publish and `apply_published_entry` swaps it in, so this only has to
/// stay stable until then. The `p` prefix keeps it from colliding with the
/// server's numeric ids.
pub fn temp_entry_id(seq: i64) -> String {
    format!("p{seq}")
}

/// Parses a temporary id back into its queue row seq.
pub fn temp_entry_seq(id: &str) -> Option<i64> {
    id.strip_prefix('p')?.parse::<i64>().ok().filter(|seq| *seq > 0)
}

/// One durable upload-queue row: the full capture payload.
#[derive(Debug)]
pub struct PendingQueueRow {
    pub seq: i64,
    pub kind: String,
    pub content: String,
    pub extra: String,
    pub created_at: String,
}

/// Durable upload queue. Rows are appended in capture order with the complete
/// entry payload, and removed by the `save_history` sweep (publish swap,
/// eviction, deletion) or an explicit acknowledge, so an offline capture
/// replays in order on the next connection.
pub fn enqueue_pending_entry(
    database_path: &Path,
    kind: &str,
    content: &str,
    extra: &str,
    created_at: &str,
) -> Result<i64, String> {
    let connection = open_history_database(database_path)?;
    connection
        .execute(
            "INSERT INTO pending_entries (kind, content, extra, created_at) VALUES (?, ?, ?, ?)",
            params![kind, content, extra, created_at],
        )
        .map_err(|error| error.to_string())?;
    Ok(connection.last_insert_rowid())
}

/// Recreates the queue row an existing temporary-id entry should have, for the
/// rare case where the entry survived but its row did not. A row already
/// occupying the seq is kept.
pub fn ensure_pending_entry(
    database_path: &Path,
    seq: i64,
    kind: &str,
    content: &str,
    extra: &str,
    created_at: &str,
) -> Result<(), String> {
    let connection = open_history_database(database_path)?;
    connection
        .execute(
            "INSERT OR IGNORE INTO pending_entries (seq, kind, content, extra, created_at) VALUES (?, ?, ?, ?, ?)",
            params![seq, kind, content, extra, created_at],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Folds updated payload (resolved content ids after hashing) back into the
/// queue row, recreating it if the sweep removed it in the meantime.
pub fn update_pending_entry(
    database_path: &Path,
    seq: i64,
    kind: &str,
    content: &str,
    extra: &str,
    created_at: &str,
) -> Result<(), String> {
    let connection = open_history_database(database_path)?;
    connection
        .execute(
            "INSERT INTO pending_entries (seq, kind, content, extra, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(seq) DO UPDATE SET kind = excluded.kind, content = excluded.content, extra = excluded.extra",
            params![seq, kind, content, extra, created_at],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn list_pending_rows(database_path: &Path) -> Result<Vec<PendingQueueRow>, String> {
    let connection = open_history_database(database_path)?;
    let mut statement = connection
        .prepare("SELECT seq, kind, content, extra, created_at FROM pending_entries ORDER BY seq ASC")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(PendingQueueRow {
                seq: row.get("seq")?,
                kind: row.get("kind")?,
                content: row.get("content")?,
                extra: row.get("extra")?,
                created_at: row.get("created_at")?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

/// Returns whether a row was actually removed.
pub fn acknowledge_pending_entry(database_path: &Path, seq: i64) -> Result<bool, String> {
    let connection = open_history_database(database_path)?;
    let changed = connection
        .execute(
            "DELETE FROM pending_entries WHERE seq = ?",
            params![seq],
        )
        .map_err(|error| error.to_string())?;
    Ok(changed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_entries_keep_capture_order_and_payload() {
        let directory = std::env::temp_dir().join(format!("cliproam-pending-test-{}", uuid::Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let mut seqs = Vec::new();
        for (kind, content) in [("text", "c"), ("text", "b"), ("image", "a")] {
            seqs.push(
                enqueue_pending_entry(&path, kind, content, "{}", "2026-01-01T00:00:00.000Z")
                    .expect("enqueue entry"),
            );
        }
        let rows = list_pending_rows(&path).expect("list pending");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].seq, seqs[0]);
        assert_eq!(rows[2].seq, seqs[2]);
        assert_eq!(rows[0].content, "c");
        assert_eq!(rows[2].kind, "image");
        assert_eq!(temp_entry_id(seqs[1]), format!("p{}", seqs[1]));
        assert_eq!(temp_entry_seq(&format!("p{}", seqs[1])), Some(seqs[1]));
        assert_eq!(temp_entry_seq(&seqs[1].to_string()), None);
        std::fs::remove_dir_all(&directory).expect("remove temporary database");
    }

    #[test]
    fn acknowledging_removes_only_the_confirmed_row() {
        let directory = std::env::temp_dir().join(format!("cliproam-pending-test-{}", uuid::Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let mut seqs = Vec::new();
        for content in ["a", "b", "c"] {
            seqs.push(
                enqueue_pending_entry(&path, "text", content, "{}", "2026-01-01T00:00:00.000Z")
                    .expect("enqueue entry"),
            );
        }
        assert!(acknowledge_pending_entry(&path, seqs[1]).expect("acknowledge entry"));
        assert!(!acknowledge_pending_entry(&path, seqs[1] + 1_000).expect("acknowledge entry"));
        let remaining = list_pending_rows(&path).expect("list pending");
        assert_eq!(remaining.iter().map(|row| row.seq).collect::<Vec<_>>(), [seqs[0], seqs[2]]);
        std::fs::remove_dir_all(&directory).expect("remove temporary database");
    }

    #[test]
    fn ensure_and_update_pending_entry_round_trip() {
        let directory = std::env::temp_dir().join(format!("cliproam-pending-test-{}", uuid::Uuid::new_v4()));
        let path = directory.join("history.sqlite");
        let seq = enqueue_pending_entry(&path, "files", "bundle", r#"{"fileInfo":{}}"#, "2026-01-01T00:00:00.000Z")
            .expect("enqueue entry");
        // Recreating an existing row keeps the original.
        ensure_pending_entry(&path, seq, "files", "bundle", r#"{"fileInfo":{}}"#, "2026-01-01T00:00:00.000Z")
            .expect("ensure entry");
        assert_eq!(list_pending_rows(&path).expect("list pending").len(), 1);
        // Hashing folds the resolved content ids back into the row.
        update_pending_entry(&path, seq, "files", "bundle", r#"{"fileInfo":{"a.txt":{"f":"aabb","s":1}}}"#, "2026-01-01T00:00:00.000Z")
            .expect("update entry");
        let rows = list_pending_rows(&path).expect("list pending");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].extra.contains("aabb"));
        // A row the sweep already removed is recreated by the update.
        acknowledge_pending_entry(&path, seq).expect("acknowledge entry");
        update_pending_entry(&path, seq, "files", "bundle", r#"{}"#, "2026-01-01T00:00:00.000Z")
            .expect("update entry again");
        assert_eq!(list_pending_rows(&path).expect("list pending").len(), 1);
        std::fs::remove_dir_all(&directory).expect("remove temporary database");
    }
}
