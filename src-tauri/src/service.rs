use std::{fs, path::{Path, PathBuf}};

use hostname::get;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::time::{timeout, Duration};
use tracing::warn;
use url::{form_urlencoded, Url};

#[cfg(target_os = "windows")]
use tauri::Emitter;
#[cfg(target_os = "windows")]
use tauri_plugin_updater::UpdaterExt;

use crate::{
    api::{DeviceListResponse, DEVICES_PATH},
    auth,
    crypto::keys::generate_key_pair,
    error::{AppError, AppResult},
    i18n::{self, TextKey},
    models::{
        access_token_timestamps, AppSettings, AppUpdateRelease, BootstrapPayload,
        DeviceDeletePayload, DeviceIdentity, DeviceInfo, DeviceNameUpdatePayload, LoginPayload,
        MusicProviderConfig, MusicProviderMeta, RegisterPayload, RotateDeviceKeyPayload,
        SessionRecord,
    },
    music::provider::KNOWN_PROVIDERS,
    shell,
    state::AppState,
};

const AUTH_LOGIN_PATH: &str = "/api/v1/auth/login";
const AUTH_LOGOUT_PATH: &str = "/api/v1/auth/logout";
const AUTH_REGISTER_PATH: &str = "/api/v1/auth/register";
const ME_PATH: &str = "/api/v1/me";
const UPDATE_CHECK_PATH: &str = "/api/v1/update/check";
#[cfg(target_os = "windows")]
const TAURI_UPDATE_PATH: &str = "/api/v1/update/tauri/windows/x86_64";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionExchangeResponse {
    user_id: String,
    token: String,
    refresh_token: String,
    expires_in: Option<i64>,
    refresh_expires_in: Option<i64>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppUpdateCheckResponse {
    has_update: bool,
    latest: Option<AppUpdateRelease>,
}

pub async fn bootstrap(state: &AppState) -> AppResult<BootstrapPayload> {
    let settings = load_settings(state)?;
    let identity = ensure_local_device_identity(state)?;
    state.runtime.activate()?;

    let session = state.database.load_session()?;
    if session.is_some() {
        state.runtime.reset_cached_device_presence()?;
    }
    let session_summary = session.map(|session| {
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
    let session = state.database.load_session()?;

    clear_auth_state(state)?;

    if let Some(session) = session {
        let request = LogoutRequest {
            refresh_token: &session.refresh_token,
        };

        let _ = timeout(
            Duration::from_secs(3),
            state.http.post_empty(
                &settings.server_url,
                AUTH_LOGOUT_PATH,
                &request,
                Some(&session.access_token),
            ),
        )
        .await;
    }

    Ok(())
}

pub async fn list_devices(state: &AppState) -> AppResult<Vec<DeviceInfo>> {
    if !state.cloud.is_connected() {
        return reconcile_or_list_devices(state);
    }

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
            state.runtime.restart_lan_after_key_rotation()?;
            if state.cloud.is_connected() {
                if let Some(session) = current_session_if_available(state).await? {
                    if identity.user_id.as_deref() == Some(session.user_id.as_str()) {
                        sync_cloud_device_key_if_pending(state, &session, &identity).await;
                    }
                }
            }
            return reconcile_or_list_devices(state);
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
    if !Path::new(&normalized.download_path).is_absolute() {
        return Err(AppError::message(user_text(
            state,
            TextKey::DownloadPathMustBeAbsolute,
        )));
    }

    Url::parse(&normalized.server_url)?;
    validate_receive_directory(&normalized.download_path)?;

    state.database.save_settings(&normalized)?;
    shell::apply_auto_start(normalized.auto_start)?;

    if state.database.load_session()?.is_some() {
        state.cloud.restart();
    } else {
        state.cloud.stop_quiet();
    }
    state.runtime.activate()?;
    shell::refresh_tray(&state.app)?;

    Ok(normalized)
}

pub(crate) fn validate_receive_directory(path: &str) -> AppResult<PathBuf> {
    let directory = PathBuf::from(path.trim());
    if directory.as_os_str().is_empty() {
        return Err(AppError::message("File receiving path cannot be empty"));
    }
    if !directory.is_absolute() {
        return Err(AppError::message("File receiving path must be an absolute path"));
    }

    fs::create_dir_all(&directory)?;
    if !fs::metadata(&directory)?.is_dir() {
        return Err(AppError::message("File receiving path is not a directory"));
    }

    let probe = directory.join(format!(
        ".colink-write-check-{}-{}",
        std::process::id(),
        crate::models::unix_now_millis(),
    ));
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)?;
    fs::remove_file(probe)?;
    Ok(directory)
}

pub fn get_music_providers(state: &AppState) -> AppResult<Vec<MusicProviderConfig>> {
    state.database.load_music_providers()
}

pub fn update_music_providers(
    state: &AppState,
    providers: Vec<MusicProviderConfig>,
) -> AppResult<()> {
    state.database.save_music_providers(&providers)?;
    state.runtime.reload_music_config();
    Ok(())
}

pub fn list_available_music_providers() -> Vec<MusicProviderMeta> {
    KNOWN_PROVIDERS
        .iter()
        .map(|provider| MusicProviderMeta {
            id: provider.id.to_string(),
            name: provider.name.to_string(),
            implemented: provider.implemented,
        })
        .collect()
}

pub async fn check_update(
    state: &AppState,
    app: &tauri::AppHandle,
) -> AppResult<Option<AppUpdateRelease>> {
    let settings = load_settings(state)?;
    let architecture = update_architecture()?;
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("platform", update_platform())
        .append_pair("arch", architecture)
        .append_pair("version", env!("CARGO_PKG_VERSION"))
        .finish();
    let path = format!("{UPDATE_CHECK_PATH}?{query}");
    let response: AppUpdateCheckResponse =
        state.http.get(&settings.server_url, &path, None).await?;
    if !response.has_update {
        return Ok(None);
    }

    let mut release = response
        .latest
        .ok_or_else(|| AppError::message("update response missing latest release"))?;
    for asset in &mut release.assets {
        asset.download_url = absolute_url(&settings.server_url, &asset.download_url)?;
    }
    release.automatic_install_available = automatic_update_available(&settings, app).await;
    Ok(Some(release))
}

pub async fn install_tauri_update(
    state: &AppState,
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
) -> AppResult<()> {
    #[cfg(target_os = "windows")]
    {
        let settings = load_settings(state)?;
        let update = check_tauri_update(&settings, app)
            .await?
            .ok_or_else(|| AppError::message("no automatic update is available"))?;

        let _ = window.emit("update-progress", 0_u8);
        let progress_window = window.clone();
        let installing_window = window.clone();
        let mut downloaded = 0_u64;
        update
            .download_and_install(
                move |chunk_length, content_length| {
                    downloaded = downloaded.saturating_add(chunk_length as u64);
                    if let Some(total) = content_length.filter(|total| *total > 0) {
                        let percent = (downloaded.saturating_mul(100) / total).min(100) as u8;
                        let _ = progress_window.emit("update-progress", percent);
                    }
                },
                move || {
                    let _ = installing_window.emit("update-installing", ());
                },
            )
            .await
            .map_err(|error| AppError::message(format!("install updater: {error}")))?;

        app.restart();
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (state, app, window);
        Err(AppError::message("automatic updates are only supported on Windows"))
    }
}

fn update_platform() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else {
        "windows"
    }
}

fn update_architecture() -> AppResult<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x64"),
        "aarch64" => Ok("arm64"),
        architecture => Err(AppError::message(format!(
            "unsupported update architecture: {architecture}"
        ))),
    }
}

#[cfg(target_os = "windows")]
async fn automatic_update_available(settings: &AppSettings, app: &tauri::AppHandle) -> bool {
    match check_tauri_update(settings, app).await {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => {
            warn!(error = %error, "check automatic update");
            false
        }
    }
}

#[cfg(not(target_os = "windows"))]
async fn automatic_update_available(_settings: &AppSettings, _app: &tauri::AppHandle) -> bool {
    false
}

#[cfg(target_os = "windows")]
async fn check_tauri_update(
    settings: &AppSettings,
    app: &tauri::AppHandle,
) -> AppResult<Option<tauri_plugin_updater::Update>> {
    let endpoint = Url::parse(&format!(
        "{}{TAURI_UPDATE_PATH}/{}",
        settings.server_url,
        env!("CARGO_PKG_VERSION"),
    ))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|error| AppError::message(format!("build updater endpoint: {error}")))?
        .build()
        .map_err(|error| AppError::message(format!("build updater: {error}")))?;
    updater
        .check()
        .await
        .map_err(|error| AppError::message(format!("check updater: {error}")))
}

pub fn open_update_download_url(url: &str) -> AppResult<()> {
    let parsed = Url::parse(url)?;
    match parsed.scheme() {
        "http" | "https" => shell::open_external_url(url),
        _ => Err(AppError::message("unsupported update download url")),
    }
}

fn load_settings(state: &AppState) -> AppResult<AppSettings> {
    state.database.load_settings()?.ok_or_else(|| {
        AppError::message(i18n::text(
            &i18n::default_language_code(),
            TextKey::SettingsNotInitialized,
        ))
    })
}

fn absolute_url(base_url: &str, value: &str) -> AppResult<String> {
    let parsed = Url::parse(value);
    if let Ok(url) = parsed {
        return Ok(url.to_string());
    }

    let base = Url::parse(base_url)?;
    Ok(base.join(value)?.to_string())
}

fn clear_auth_state(state: &AppState) -> AppResult<()> {
    state.cloud.stop_quiet();
    state.database.clear_session()?;
    state.database.clear_cached_devices()?;
    state.database.clear_cloud_trust()?;
    let _ = publish_offline_devices(state);
    shell::refresh_tray(&state.app)?;
    Ok(())
}

async fn save_session_and_bootstrap(
    state: &AppState,
    settings: AppSettings,
    response: SessionExchangeResponse,
) -> AppResult<BootstrapPayload> {
    let _ = response.refresh_expires_in;
    let (access_token_expires_at, access_token_refresh_at) =
        access_token_timestamps(response.expires_in);
    let session = SessionRecord {
        user_id: response.user_id,
        username: String::new(),
        access_token: response.token,
        refresh_token: response.refresh_token,
        access_token_expires_at,
        access_token_refresh_at,
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
        local_ip: None,
        local_port: None,
        lan_available: false,
        lan_state: "unavailable".to_string(),
        active_route: None,
        device_sources: vec!["local".to_string()],
        trusted_by_lan: false,
        trusted_by_cloud: false,
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
        AppError::Protocol { code, .. } => AppError::is_auth_protocol_code(*code),
        AppError::Message(message) => {
            message.eq_ignore_ascii_case("unauthorized")
                || message.eq_ignore_ascii_case("invalid refresh token")
                || message.eq_ignore_ascii_case("token revoked")
        }
        _ => false,
    }
}

fn detect_device_name() -> String {
    detect_device_name_from(
        get().ok().and_then(|value| value.into_string().ok()).as_deref(),
        cfg!(debug_assertions),
    )
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

#[cfg(test)]
mod tests {
    use super::{detect_device_name_from, validate_receive_directory};

    #[test]
    fn appends_debug_suffix_in_debug_builds() {
        let name = detect_device_name_from(Some("CoLink"), true);
        assert_eq!(name, "CoLinkDebug");
    }

    #[test]
    fn falls_back_to_default_name_and_applies_suffix() {
        let name = detect_device_name_from(None, true);
        assert_eq!(name, "CoLink DesktopDebug");
    }

    #[test]
    fn rejects_relative_receive_directory() {
        assert!(validate_receive_directory("1321123321").is_err());
    }
}

fn detect_device_name_from(system_name: Option<&str>, debug_build: bool) -> String {
    let base_name = system_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("CoLink Desktop");

    if debug_build {
        format!("{base_name}Debug")
    } else {
        base_name.to_string()
    }
}
