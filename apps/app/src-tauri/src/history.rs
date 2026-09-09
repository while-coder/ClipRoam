//! History-level state that is not per-entry. Entries live in SQLite and are
//! handled by the `entry` module; what remains here is the device identity the
//! frontend shows alongside every entry.

use tauri::State;

use crate::AppState;

#[tauri::command]
pub(crate) fn get_device(state: State<'_, AppState>) -> Result<(String, String), String> {
    let history = state.history.lock().map_err(|error| error.to_string())?;
    Ok((history.device_id.clone(), history.device_name.clone()))
}
