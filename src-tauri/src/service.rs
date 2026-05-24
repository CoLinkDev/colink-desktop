use hostname::get;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    crypto::keys::generate_key_pair,
    error::{AppError, AppResult},
    models::{
        unix_now, AppSettings, BootstrapPayload, DeviceDeletePayload, DeviceIdentity,
        DeviceInfo, DeviceNameUpdatePayload, LoginPayload, RegisterPayload,
        RotateDeviceKeyPayload, SessionRecord,
    },
    shell,
    state::AppState,
};

const AUTH_LOGIN_PATH: &str = "/api/v1/auth/login";
const AUTH_LOGOUT_PATH: &str = "/api/v1/auth/logout";
const AUTH_REFRESH_PATH: &str = "/api/v1/auth/refresh";
const AUTH_REGISTER_PATH: &str = "/api/v1/auth/register";
const DEVICES_PATH: &str = "/api/v1/devices";
const ACCESS_TOKEN_TTL_SECONDS: i64 = 15 * 60;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionExchangeResponse {
    user_id: String,
    token: String,
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshResponse {
    token: String,
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRegisterResponse {
    device_id: String,
    device_secret: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceListResponse {
    devices: Vec<DeviceInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogoutRequest<'a> {
    refresh_token: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRegisterRequest<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    device_type: &'a str,
    public_key: &'a str,
}

pub async fn bootstrap(state: &AppState) -> AppResult<BootstrapPayload> {
    let settings = load_settings(state)?;
    let stored_device = state.database.load_device_identity()?;
    let stored_session = state.database.load_session()?;

    let mut session_summary = None;
    let mut devices = Vec::new();
    let mut device_summary = stored_device.as_ref().map(DeviceIdentity::summary);

    if let Some(session) = stored_session {
        match refresh_session(state, &session).await {
            Ok(refreshed_session) => {
                state.database.save_session(&refreshed_session)?;
                let identity = ensure_device_identity(state, &refreshed_session).await?;
                let fetched_devices = fetch_devices(state, &refreshed_session)
                    .await
                    .unwrap_or_else(|_| state.database.load_cached_devices().unwrap_or_default());

                state.database.save_cached_devices(&fetched_devices)?;
                state.cloud.start();
                let _ = state.runtime.activate();
                let _ = shell::refresh_tray(&state.app);

                session_summary = Some(refreshed_session.summary());
                device_summary = Some(identity.summary());
                devices = fetched_devices;
            }
            Err(_) => {
                clear_auth_state(state)?;
            }
        }
    } else {
        state.cloud.stop();
        let _ = state.runtime.deactivate();
    }

    Ok(BootstrapPayload {
        settings,
        session: session_summary,
        devices,
        device: device_summary,
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
    let session = current_session(state).await?;
    let devices = fetch_devices(state, &session).await?;
    state.database.save_cached_devices(&devices)?;
    shell::refresh_tray(&state.app)?;
    Ok(devices)
}

pub async fn update_device_name(
    state: &AppState,
    payload: DeviceNameUpdatePayload,
) -> AppResult<Vec<DeviceInfo>> {
    let session = current_session(state).await?;
    let settings = load_settings(state)?;
    let path = format!("{DEVICES_PATH}/{}", payload.device_id);
    let request = serde_json::json!({ "name": payload.name });
    state
        .http
        .put_empty(&settings.server_url, &path, &request, Some(&session.access_token))
        .await?;

    if let Some(mut identity) = state.database.load_device_identity()? {
        if identity.device_id == payload.device_id {
            identity.name = payload.name;
            state.database.save_device_identity(&identity)?;
        }
    }

    let devices = fetch_devices(state, &session).await?;
    state.database.save_cached_devices(&devices)?;
    shell::refresh_tray(&state.app)?;
    Ok(devices)
}

pub async fn delete_device(
    state: &AppState,
    payload: DeviceDeletePayload,
) -> AppResult<Vec<DeviceInfo>> {
    let session = current_session(state).await?;
    let settings = load_settings(state)?;
    let path = format!("{DEVICES_PATH}/{}", payload.device_id);
    state
        .http
        .delete_empty(&settings.server_url, &path, Some(&session.access_token))
        .await?;

    if let Some(identity) = state.database.load_device_identity()? {
        if identity.device_id == payload.device_id {
            state.database.clear_device_identity()?;
            clear_auth_state(state)?;
            return Ok(Vec::new());
        }
    }

    let devices = fetch_devices(state, &session).await?;
    state.database.save_cached_devices(&devices)?;
    shell::refresh_tray(&state.app)?;
    Ok(devices)
}

pub async fn rotate_device_key(
    state: &AppState,
    payload: RotateDeviceKeyPayload,
) -> AppResult<Vec<DeviceInfo>> {
    let session = current_session(state).await?;
    let settings = load_settings(state)?;
    let generated = generate_key_pair()?;
    let path = format!("{DEVICES_PATH}/{}/key", payload.device_id);
    let request = serde_json::json!({ "publicKey": generated.public_key });
    state
        .http
        .put_empty(&settings.server_url, &path, &request, Some(&session.access_token))
        .await?;

    if let Some(mut identity) = state.database.load_device_identity()? {
        if identity.device_id == payload.device_id {
            identity.public_key = generated.public_key.clone();
            identity.private_key = generated.private_key;
            state.database.save_device_identity(&identity)?;
        }
    }

    let devices = fetch_devices(state, &session).await?;
    state.database.save_cached_devices(&devices)?;
    shell::refresh_tray(&state.app)?;
    Ok(devices)
}

pub fn get_settings(state: &AppState) -> AppResult<AppSettings> {
    load_settings(state)
}

pub fn update_settings(state: &AppState, settings: AppSettings) -> AppResult<AppSettings> {
    let normalized = settings.normalize();

    if normalized.download_path.is_empty() {
        return Err(AppError::message("下载路径不能为空"));
    }

    Url::parse(&normalized.server_url)?;

    state.database.save_settings(&normalized)?;
    shell::apply_auto_start(normalized.auto_start)?;

    if state.database.load_session()?.is_some() {
        state.cloud.restart();
        let _ = state.runtime.activate();
    }
    shell::refresh_tray(&state.app)?;

    Ok(normalized)
}

fn load_settings(state: &AppState) -> AppResult<AppSettings> {
    state
        .database
        .load_settings()?
        .ok_or_else(|| AppError::message("本地设置未初始化"))
}

fn clear_auth_state(state: &AppState) -> AppResult<()> {
    state.cloud.stop();
    state.runtime.deactivate()?;
    state.database.clear_session()?;
    state.database.clear_cached_devices()?;
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
        access_token: response.token,
        refresh_token: response.refresh_token,
        access_token_expires_at: unix_now() + ACCESS_TOKEN_TTL_SECONDS,
    };

    state.database.save_session(&session)?;
    let identity = ensure_device_identity(state, &session).await?;
    let devices = fetch_devices(state, &session).await?;
    state.database.save_cached_devices(&devices)?;
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
        .ok_or_else(|| AppError::message("尚未登录"))?;

    if !session.is_expiring_soon() {
        return Ok(session);
    }

    let refreshed = refresh_session(state, &session).await?;
    state.database.save_session(&refreshed)?;
    Ok(refreshed)
}

async fn refresh_session(state: &AppState, session: &SessionRecord) -> AppResult<SessionRecord> {
    let settings = load_settings(state)?;
    let request = RefreshRequest {
        refresh_token: &session.refresh_token,
    };

    let response: RefreshResponse = state
        .http
        .post(&settings.server_url, AUTH_REFRESH_PATH, &request, None)
        .await?;

    Ok(SessionRecord {
        user_id: session.user_id.clone(),
        access_token: response.token,
        refresh_token: response.refresh_token,
        access_token_expires_at: unix_now() + ACCESS_TOKEN_TTL_SECONDS,
    })
}

async fn ensure_device_identity(
    state: &AppState,
    session: &SessionRecord,
) -> AppResult<DeviceIdentity> {
    if let Some(identity) = state.database.load_device_identity()? {
        if identity.user_id == session.user_id {
            return Ok(identity);
        }
    }

    let generated = generate_key_pair()?;
    let name = detect_device_name();
    let device_type = detect_device_type();
    let settings = load_settings(state)?;
    let request = DeviceRegisterRequest {
        name: &name,
        device_type: &device_type,
        public_key: &generated.public_key,
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

    let identity = DeviceIdentity {
        user_id: session.user_id.clone(),
        device_id: response.device_id,
        device_secret: response.device_secret,
        name,
        device_type,
        public_key: generated.public_key,
        private_key: generated.private_key,
    };

    state.database.save_device_identity(&identity)?;
    Ok(identity)
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

    Ok(response.devices)
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
