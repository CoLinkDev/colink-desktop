use hostname::get;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::warn;
use url::Url;

use crate::{
    api::{DeviceListResponse, ACCESS_TOKEN_TTL_SECONDS, DEVICES_PATH},
    auth,
    crypto::keys::generate_key_pair,
    error::{AppError, AppResult},
    i18n::{self, TextKey},
    models::{
        unix_now, AppSettings, BootstrapPayload, DeviceDeletePayload, DeviceIdentity, DeviceInfo,
        DeviceNameUpdatePayload, LoginPayload, RegisterPayload, RotateDeviceKeyPayload,
        SessionRecord,
    },
    shell,
    state::AppState,
};

const AUTH_LOGIN_PATH: &str = "/api/v1/auth/login";
const AUTH_LOGOUT_PATH: &str = "/api/v1/auth/logout";
const AUTH_REGISTER_PATH: &str = "/api/v1/auth/register";
const ME_PATH: &str = "/api/v1/me";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionExchangeResponse {
    user_id: String,
    token: String,
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeResponse {
    user_id: String,
    username: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRegisterResponse {
    device_id: String,
    device_secret: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogoutRequest<'a> {
    refresh_token: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRegisterRequest<'a> {
    device_id: &'a str,
    name: &'a str,
    #[serde(rename = "type")]
    device_type: &'a str,
    public_key: &'a str,
}

pub async fn bootstrap(state: &AppState) -> AppResult<BootstrapPayload> {
    let settings = load_settings(state)?;
    let identity = ensure_local_device_identity(state)?;
    state.runtime.activate()?;

    let session_summary = state.database.load_session()?.map(|session| {
        state.cloud.start();
        let _ = shell::refresh_tray(&state.app);
        session.summary()
    });
    let devices = if session_summary.is_some() {
        state.runtime.reconcile_device_routes()?
    } else {
        state.cloud.stop_quiet();
        publish_offline_devices(state)?
    };

    Ok(BootstrapPayload {
        settings,
        session: session_summary,
        devices,
        device: Some(identity.summary()),
        cloud: state.cloud.snapshot(),
        messages: state.database.load_messages(200)?,
        transfers: state.database.load_transfers(200)?,
        logs: state.database.load_logs(200)?,
    })
}

pub async fn login(state: &AppState, payload: LoginPayload) -> AppResult<BootstrapPayload> {
    let settings = load_settings(state)?;
    let response: SessionExchangeResponse = state
        .http
        .post(&settings.server_url, AUTH_LOGIN_PATH, &payload, None)
        .await?;

    save_session_and_bootstrap(state, settings, response).await
}

pub async fn register_account(
    state: &AppState,
    payload: RegisterPayload,
) -> AppResult<BootstrapPayload> {
    let settings = load_settings(state)?;
    let response: SessionExchangeResponse = state
        .http
        .post(&settings.server_url, AUTH_REGISTER_PATH, &payload, None)
        .await?;

    save_session_and_bootstrap(state, settings, response).await
}

pub async fn logout(state: &AppState) -> AppResult<()> {
    let settings = load_settings(state)?;

    if let Some(session) = state.database.load_session()? {
        let request = LogoutRequest {
            refresh_token: &session.refresh_token,
        };

        let _ = state
            .http
            .post_empty(
                &settings.server_url,
                AUTH_LOGOUT_PATH,
                &request,
                Some(&session.access_token),
            )
            .await;
    }

    clear_auth_state(state)
}

pub async fn list_devices(state: &AppState) -> AppResult<Vec<DeviceInfo>> {
    let Some(session) = current_session_or_clear(state).await? else {
        return reconcile_or_list_devices(state);
    };
    let devices = match fetch_devices(state, &session).await {
        Ok(devices) => state.runtime.replace_cached_devices(devices, true)?,
        Err(error) => {
            warn!(%error, "failed to fetch cloud devices");
            state.runtime.reconcile_device_routes()?
        }
    };
    shell::refresh_tray(&state.app)?;
    Ok(devices)
}

pub async fn update_device_name(
    state: &AppState,
    payload: DeviceNameUpdatePayload,
) -> AppResult<Vec<DeviceInfo>> {
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::message(user_text(
            state,
            TextKey::DeviceNameEmpty,
        )));
    }

    if let Some(mut identity) = state.database.load_device_identity()? {
        if identity.device_id == payload.device_id {
            identity.name = name;
            state.database.save_device_identity(&identity)?;
            state.runtime.activate()?;
            if let Some(session) = current_session_if_available(state).await? {
                if identity.user_id.as_deref() == Some(session.user_id.as_str()) {
                    if let Err(error) = sync_cloud_device_name(state, &session, &identity).await {
                        warn!(%error, "failed to sync local device name to cloud");
                    }
                }
            }
            return reconcile_or_list_devices(state);
        }
    }

    let session = current_session(state).await?;
    let settings = load_settings(state)?;
    let path = format!("{DEVICES_PATH}/{}", payload.device_id);
    let request = serde_json::json!({ "name": name });
    state
        .http
        .put_empty(
            &settings.server_url,
            &path,
            &request,
            Some(&session.access_token),
        )
        .await?;

    let devices = fetch_devices(state, &session).await?;
    let devices = state.runtime.replace_cached_devices(devices, true)?;
    shell::refresh_tray(&state.app)?;
    Ok(devices)
}

pub async fn delete_device(
    state: &AppState,
    payload: DeviceDeletePayload,
) -> AppResult<Vec<DeviceInfo>> {
    if let Some(identity) = state.database.load_device_identity()? {
        if identity.device_id == payload.device_id {
            return Err(AppError::message(user_text(
                state,
                TextKey::CannotDeleteLocalDevice,
            )));
        }
    }

    let session = current_session(state).await?;
    let settings = load_settings(state)?;
    let path = format!("{DEVICES_PATH}/{}", payload.device_id);
    state
        .http
        .delete_empty(&settings.server_url, &path, Some(&session.access_token))
        .await?;

    let devices = fetch_devices(state, &session).await?;
    let devices = state.runtime.replace_cached_devices(devices, true)?;
    shell::refresh_tray(&state.app)?;
    Ok(devices)
}

pub async fn rotate_device_key(
    state: &AppState,
    payload: RotateDeviceKeyPayload,
) -> AppResult<Vec<DeviceInfo>> {
    let generated = generate_key_pair()?;

    if let Some(mut identity) = state.database.load_device_identity()? {
        if identity.device_id == payload.device_id {
            identity.public_key = generated.public_key.clone();
            identity.private_key = generated.private_key;
            identity.cloud_key_sync_pending = true;
            state.database.save_device_identity(&identity)?;
            if let Some(session) = current_session_if_available(state).await? {
                if identity.user_id.as_deref() == Some(session.user_id.as_str()) {
                    sync_cloud_device_key_if_pending(state, &session, &identity).await;
                }
            }
            return list_devices(state).await;
        }
    }

    let session = current_session(state).await?;
    let settings = load_settings(state)?;
    let path = format!("{DEVICES_PATH}/{}/key", payload.device_id);
    let request = serde_json::json!({ "publicKey": generated.public_key });
    state
        .http
        .put_empty(
            &settings.server_url,
            &path,
            &request,
            Some(&session.access_token),
        )
        .await?;

    let devices = fetch_devices(state, &session).await?;
    let devices = state.runtime.replace_cached_devices(devices, true)?;
    shell::refresh_tray(&state.app)?;
    Ok(devices)
}

pub fn get_settings(state: &AppState) -> AppResult<AppSettings> {
    load_settings(state)
}

pub fn update_settings(state: &AppState, settings: AppSettings) -> AppResult<AppSettings> {
    let normalized = settings.normalize();

    if normalized.download_path.is_empty() {
        return Err(AppError::message(user_text(
            state,
            TextKey::DownloadPathEmpty,
        )));
    }

    Url::parse(&normalized.server_url)?;

    state.database.save_settings(&normalized)?;
    shell::apply_auto_start(normalized.auto_start)?;

    if state.database.load_session()?.is_some() {
        state.cloud.restart();
    } else {
        state.cloud.stop_quiet();
    }
    if normalized.lan_discovery {
        state.runtime.activate()?;
    } else {
        state.runtime.deactivate()?;
    }
    shell::refresh_tray(&state.app)?;

    Ok(normalized)
}

fn load_settings(state: &AppState) -> AppResult<AppSettings> {
    state.database.load_settings()?.ok_or_else(|| {
        AppError::message(i18n::text(
            &i18n::default_language_code(),
            TextKey::SettingsNotInitialized,
        ))
    })
}

fn clear_auth_state(state: &AppState) -> AppResult<()> {
    state.cloud.stop_quiet();
    state.database.clear_session()?;
    state.database.clear_cached_devices()?;
    let _ = publish_offline_devices(state);
    shell::refresh_tray(&state.app)?;
    Ok(())
}

async fn save_session_and_bootstrap(
    state: &AppState,
    settings: AppSettings,
    response: SessionExchangeResponse,
) -> AppResult<BootstrapPayload> {
    let session = SessionRecord {
        user_id: response.user_id,
        username: String::new(),
        access_token: response.token,
        refresh_token: response.refresh_token,
        access_token_expires_at: unix_now() + ACCESS_TOKEN_TTL_SECONDS,
    };
    let session = session_with_profile(state, &settings, &session).await?;

    let identity = ensure_cloud_device_identity(state, &session).await?;
    state.database.save_session(&session)?;
    let devices = fetch_devices(state, &session).await?;
    let devices = state.runtime.replace_cached_devices(devices, true)?;
    state.cloud.start();
    let _ = state.runtime.activate();
    let _ = shell::refresh_tray(&state.app);

    Ok(BootstrapPayload {
        settings,
        session: Some(session.summary()),
        devices,
        device: Some(identity.summary()),
        cloud: state.cloud.snapshot(),
        messages: state.database.load_messages(200)?,
        transfers: state.database.load_transfers(200)?,
        logs: state.database.load_logs(200)?,
    })
}

async fn current_session(state: &AppState) -> AppResult<SessionRecord> {
    let session = state
        .database
        .load_session()?
        .ok_or_else(|| AppError::message(user_text(state, TextKey::NotLoggedIn)))?;
    let settings = load_settings(state)?;

    auth::refresh_session_if_needed(&state.database, &state.http, &settings, session).await
}

async fn session_with_profile(
    state: &AppState,
    settings: &AppSettings,
    session: &SessionRecord,
) -> AppResult<SessionRecord> {
    let profile: MeResponse = state
        .http
        .get(&settings.server_url, ME_PATH, Some(&session.access_token))
        .await?;

    let mut next = session.clone();
    next.user_id = profile.user_id;
    next.username = profile.username.trim().to_string();
    Ok(next)
}

async fn current_session_or_clear(state: &AppState) -> AppResult<Option<SessionRecord>> {
    if state.database.load_session()?.is_none() {
        return Ok(None);
    }

    match current_session(state).await {
        Ok(session) => Ok(Some(session)),
        Err(error) if is_auth_error(&error) => {
            warn!(%error, "clearing invalid cloud session");
            clear_auth_state(state)?;
            Ok(None)
        }
        Err(error) => {
            warn!(%error, "cloud session unavailable");
            Ok(None)
        }
    }
}

async fn current_session_if_available(state: &AppState) -> AppResult<Option<SessionRecord>> {
    if state.database.load_session()?.is_none() {
        return Ok(None);
    }

    match current_session(state).await {
        Ok(session) => Ok(Some(session)),
        Err(error) if is_auth_error(&error) => {
            warn!(%error, "clearing invalid cloud session");
            clear_auth_state(state)?;
            Ok(None)
        }
        Err(error) => {
            warn!(%error, "cloud session unavailable");
            Ok(None)
        }
    }
}

fn ensure_local_device_identity(state: &AppState) -> AppResult<DeviceIdentity> {
    if let Some(identity) = state.database.load_device_identity()? {
        return Ok(identity);
    }

    let generated = generate_key_pair()?;
    let identity = DeviceIdentity {
        user_id: None,
        device_id: uuid::Uuid::new_v4().to_string(),
        device_secret: None,
        name: detect_device_name(),
        device_type: detect_device_type(),
        public_key: generated.public_key,
        private_key: generated.private_key,
        cloud_key_sync_pending: false,
    };
    state.database.save_device_identity(&identity)?;
    Ok(identity)
}

async fn ensure_cloud_device_identity(
    state: &AppState,
    session: &SessionRecord,
) -> AppResult<DeviceIdentity> {
    let mut identity = ensure_local_device_identity(state)?;
    match identity.user_id.as_deref() {
        Some(user_id) if user_id == session.user_id && identity.device_secret.is_some() => {
            sync_cloud_device_identity(state, session, &identity).await;
            return Ok(identity);
        }
        Some(user_id) if user_id != session.user_id => {
            return Err(AppError::message(user_message(
                state,
                TextKey::LocalIdentityBoundOtherAccount,
                &[("user_id", user_id.to_string())],
            )));
        }
        _ => {}
    }

    let settings = load_settings(state)?;
    let request = DeviceRegisterRequest {
        device_id: &identity.device_id,
        name: &identity.name,
        device_type: &identity.device_type,
        public_key: &identity.public_key,
    };

    let response: DeviceRegisterResponse = state
        .http
        .post(
            &settings.server_url,
            DEVICES_PATH,
            &request,
            Some(&session.access_token),
        )
        .await?;

    identity.user_id = Some(session.user_id.clone());
    identity.device_id = response.device_id;
    identity.device_secret = Some(response.device_secret);
    state.database.save_device_identity(&identity)?;
    sync_cloud_device_identity(state, session, &identity).await;
    Ok(identity)
}

async fn sync_cloud_device_identity(
    state: &AppState,
    session: &SessionRecord,
    identity: &DeviceIdentity,
) {
    if let Err(error) = sync_cloud_device_name(state, session, identity).await {
        warn!(%error, "failed to sync local device name to cloud");
    }
    sync_cloud_device_key_if_pending(state, session, identity).await;
}

async fn sync_cloud_device_name(
    state: &AppState,
    session: &SessionRecord,
    identity: &DeviceIdentity,
) -> AppResult<()> {
    let settings = load_settings(state)?;
    let path = format!("{DEVICES_PATH}/{}", identity.device_id);
    let request = serde_json::json!({ "name": identity.name });
    state
        .http
        .put_empty(
            &settings.server_url,
            &path,
            &request,
            Some(&session.access_token),
        )
        .await
}

async fn sync_cloud_device_key(
    state: &AppState,
    session: &SessionRecord,
    identity: &DeviceIdentity,
) -> AppResult<()> {
    let settings = load_settings(state)?;
    let path = format!("{DEVICES_PATH}/{}/key", identity.device_id);
    let request = serde_json::json!({ "publicKey": identity.public_key });
    state
        .http
        .put_empty(
            &settings.server_url,
            &path,
            &request,
            Some(&session.access_token),
        )
        .await
}

fn reconcile_or_list_devices(state: &AppState) -> AppResult<Vec<DeviceInfo>> {
    match state.database.load_session()? {
        Some(_) => state.runtime.reconcile_device_routes(),
        None => publish_offline_devices(state),
    }
}

async fn sync_cloud_device_key_if_pending(
    state: &AppState,
    session: &SessionRecord,
    identity: &DeviceIdentity,
) {
    if !identity.cloud_key_sync_pending {
        return;
    }

    if let Err(error) = sync_cloud_device_key(state, session, identity).await {
        warn!(%error, "failed to sync local device key to cloud");
        return;
    }

    match state.database.load_device_identity() {
        Ok(Some(mut latest))
            if latest.device_id == identity.device_id
                && latest.public_key == identity.public_key =>
        {
            latest.cloud_key_sync_pending = false;
            if let Err(error) = state.database.save_device_identity(&latest) {
                warn!(%error, "failed to clear pending device key sync");
            }
        }
        Ok(_) => {}
        Err(error) => warn!(%error, "failed to reload device identity after key sync"),
    }
}

fn publish_offline_devices(state: &AppState) -> AppResult<Vec<DeviceInfo>> {
    let identity = ensure_local_device_identity(state)?;
    state
        .runtime
        .replace_cached_devices(vec![local_device_info(&identity)], false)
}

fn local_device_info(identity: &DeviceIdentity) -> DeviceInfo {
    DeviceInfo {
        device_id: identity.device_id.clone(),
        name: identity.name.clone(),
        device_type: identity.device_type.clone(),
        online: true,
        cloud_available: false,
        last_seen: None,
        public_key: identity.public_key.clone(),
        public_key_updated_at: None,
        lan_available: false,
        active_route: None,
        device_sources: vec!["local".to_string()],
        security_state: "verified".to_string(),
    }
}

async fn fetch_devices(state: &AppState, session: &SessionRecord) -> AppResult<Vec<DeviceInfo>> {
    let settings = load_settings(state)?;
    let response: DeviceListResponse = state
        .http
        .get(
            &settings.server_url,
            DEVICES_PATH,
            Some(&session.access_token),
        )
        .await?;

    Ok(response.into_devices())
}

fn is_auth_error(error: &AppError) -> bool {
    match error {
        AppError::Network(network) => network.status() == Some(StatusCode::UNAUTHORIZED),
        AppError::Message(message) => {
            message.eq_ignore_ascii_case("unauthorized")
                || message.eq_ignore_ascii_case("invalid refresh token")
                || message.eq_ignore_ascii_case("token revoked")
        }
        _ => false,
    }
}

fn detect_device_name() -> String {
    get()
        .ok()
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "CoLink Desktop".to_string())
}

fn detect_device_type() -> String {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        "linux" => "linux",
        "android" => "android",
        "ios" => "ios",
        _ => "windows",
    }
    .to_string()
}

fn settings_language(state: &AppState) -> String {
    state
        .database
        .load_settings()
        .ok()
        .flatten()
        .map(|settings| settings.language)
        .unwrap_or_else(i18n::default_language_code)
}

fn user_text(state: &AppState, key: TextKey) -> String {
    let language = settings_language(state);
    i18n::text(&language, key).to_string()
}

fn user_message(state: &AppState, key: TextKey, args: &[(&str, String)]) -> String {
    let language = settings_language(state);
    i18n::message(&language, key, args)
}
