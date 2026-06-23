use tauri::State;

use crate::{
    models::{LogPagePayload, LogPageResult},
    state::AppState,
};

const MIN_LOG_PAGE_SIZE: usize = 1;
const MAX_LOG_PAGE_SIZE: usize = 100;

#[tauri::command]
pub fn list_logs(
    state: State<'_, AppState>,
    payload: LogPagePayload,
) -> Result<LogPageResult, String> {
    let page = payload.page.max(1);
    let page_size = payload.page_size.clamp(MIN_LOG_PAGE_SIZE, MAX_LOG_PAGE_SIZE);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let total = state
        .database
        .count_logs()
        .map_err(|error| error.to_string())?;
    let logs = state
        .database
        .load_logs_page(page_size, offset)
        .map_err(|error| error.to_string())?;

    Ok(LogPageResult { logs, total })
}
