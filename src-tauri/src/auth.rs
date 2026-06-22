use crate::{
    api::{RefreshRequest, RefreshResponse, AUTH_REFRESH_PATH},
    error::AppResult,
    models::{access_token_timestamps, AppSettings, SessionRecord},
    network::http::HttpClient,
    store::db::Database,
};

pub async fn refresh_session_if_needed(
    database: &Database,
    http: &HttpClient,
    settings: &AppSettings,
    session: SessionRecord,
) -> AppResult<SessionRecord> {
    if !session.is_expiring_soon() {
        return Ok(session);
    }

    let request = RefreshRequest {
        refresh_token: &session.refresh_token,
    };

    let response: RefreshResponse = http
        .post(&settings.server_url, AUTH_REFRESH_PATH, &request, None)
        .await?;
    let _ = response.refresh_expires_in;
    let (access_token_expires_at, access_token_refresh_at) =
        access_token_timestamps(response.expires_in);

    let refreshed = SessionRecord {
        user_id: session.user_id,
        username: session.username,
        access_token: response.token,
        refresh_token: response.refresh_token,
        access_token_expires_at,
        access_token_refresh_at,
    };

    database.save_session(&refreshed)?;
    Ok(refreshed)
}
