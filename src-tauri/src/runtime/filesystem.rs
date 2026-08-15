use std::{
    fs::{self, Metadata},
    io,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{de::DeserializeOwned, Serialize};
use tauri::Emitter;
use tokio::{
    sync::oneshot,
    time::{sleep, timeout},
};
use tracing::{debug, info, warn};
use uuid::Uuid;

#[cfg(windows)]
use windows::{core::PCWSTR, Win32::Storage::FileSystem::GetVolumeInformationW};

use crate::{
    error::{AppError, AppResult},
    models::{unix_now_millis, RemoteFilesystemDownload, RemoteFilesystemUpload},
    protocol::{
        BusinessEnvelope, FsDownloadPayload, FsEntry, FsErrorPayload, FsListPayload,
        FsListResultPayload, FsRootEntry, FsRootsPayload, FsRootsResultPayload, FsStatPayload,
        FsStatResultPayload, FsUploadPayload, FsUploadReadyPayload, FS_DOWNLOAD_TYPE, FS_ERROR_TYPE,
        FS_LIST_RESULT_TYPE, FS_LIST_TYPE, FS_ROOTS_RESULT_TYPE, FS_ROOTS_TYPE, FS_STAT_RESULT_TYPE,
        FS_STAT_TYPE, FS_UPLOAD_READY_TYPE, FS_UPLOAD_TYPE,
    },
    sync::MutexExt,
};

use super::{
    AppRuntime, PendingFilesystemRequest, PendingFilesystemUpload, FILESYSTEM_DOWNLOAD_OFFER_TIMEOUT,
    FILESYSTEM_REQUEST_TIMEOUT, REMOTE_FILESYSTEM_DOWNLOADS_UPDATED_EVENT,
    REMOTE_FILESYSTEM_UPLOADS_UPDATED_EVENT,
};

const DEFAULT_LIST_LIMIT: i64 = 200;
const MAX_LIST_LIMIT: i64 = 1_000;
pub const REMOTE_FILESYSTEM_UNSUPPORTED_ERROR: &str = "colink:filesystem.unsupported.v1";
pub const REMOTE_FILESYSTEM_UPLOAD_UNSUPPORTED_ERROR: &str = "colink:filesystem.upload_unsupported.v1";

#[derive(Debug, Clone)]
pub(crate) struct FilesystemUploadDestination {
    pub(super) parent: PathBuf,
    pub(super) target: PathBuf,
}

#[derive(Debug)]
pub(super) struct FilesystemError {
    pub(super) reason: &'static str,
    pub(super) message: String,
}

impl FilesystemError {
    fn generic(message: impl Into<String>) -> Self {
        Self {
            reason: "generic",
            message: message.into(),
        }
    }

    fn from_io(error: io::Error) -> Self {
        let reason = match error.kind() {
            io::ErrorKind::NotFound => "not_found",
            io::ErrorKind::PermissionDenied => "permission_denied",
            _ => "io_error",
        };
        Self {
            reason,
            message: error.to_string(),
        }
    }
}

impl AppRuntime {
    pub async fn list_remote_filesystem_roots(
        &self,
        device_id: &str,
    ) -> AppResult<FsRootsResultPayload> {
        self.require_remote_filesystem_support(device_id)?;
        let response = self
            .request_remote_filesystem(
                device_id,
                BusinessEnvelope::from_payload(FS_ROOTS_TYPE, FsRootsPayload {})?,
                FS_ROOTS_RESULT_TYPE,
            )
            .await?;
        serde_json::from_value(response.payload).map_err(AppError::from)
    }

    pub async fn list_remote_filesystem(
        &self,
        device_id: &str,
        path: String,
        offset: Option<i64>,
    ) -> AppResult<FsListResultPayload> {
        self.require_remote_filesystem_support(device_id)?;
        let response = self
            .request_remote_filesystem(
                device_id,
                BusinessEnvelope::from_payload(
                    FS_LIST_TYPE,
                    FsListPayload {
                        path,
                        offset,
                        limit: None,
                    },
                )?,
                FS_LIST_RESULT_TYPE,
            )
            .await?;
        serde_json::from_value(response.payload).map_err(AppError::from)
    }

    pub async fn download_remote_filesystem_file(
        &self,
        device_id: &str,
        path: String,
    ) -> AppResult<RemoteFilesystemDownload> {
        self.require_remote_filesystem_support(device_id)?;

        let download = RemoteFilesystemDownload {
            request_id: Uuid::new_v4().to_string(),
            device_id: device_id.to_string(),
            remote_path: path.clone(),
            requested_at: unix_now_millis(),
            session_id: None,
            error: None,
        };
        self.remember_remote_filesystem_download(download.clone());

        let envelope = BusinessEnvelope::from_payload(FS_DOWNLOAD_TYPE, FsDownloadPayload { path })?;
        if let Err(error) = self
            .send_business_message_with_envelope_id(device_id, envelope, download.request_id.clone())
            .await
        {
            self.remove_remote_filesystem_download(&download.request_id);
            return Err(error);
        }

        let runtime = self.clone();
        let request_id = download.request_id.clone();
        tauri::async_runtime::spawn(async move {
            sleep(FILESYSTEM_DOWNLOAD_OFFER_TIMEOUT).await;
            runtime.expire_remote_filesystem_download(&request_id);
        });
        info!(%device_id, request_id = %download.request_id, "remote filesystem download requested");
        Ok(download)
    }

    pub fn upload_remote_filesystem_file(
        &self,
        device_id: &str,
        path: String,
        source_path: PathBuf,
    ) -> AppResult<RemoteFilesystemUpload> {
        self.require_remote_filesystem_upload_support(device_id)?;
        if !source_path.is_file() {
            return Err(AppError::message("Selected file no longer exists"));
        }

        let upload = RemoteFilesystemUpload {
            request_id: Uuid::new_v4().to_string(),
            device_id: device_id.to_string(),
            remote_path: path.clone(),
            requested_at: unix_now_millis(),
            session_id: None,
            error: None,
        };
        self.remember_remote_filesystem_upload(upload.clone());

        let runtime = self.clone();
        let upload_id = upload.request_id.clone();
        let device_id = device_id.to_string();
        tauri::async_runtime::spawn(async move {
            let result = async {
                let (request_id, _) = runtime
                    .request_remote_filesystem_with_id(
                        &device_id,
                        BusinessEnvelope::from_payload(FS_UPLOAD_TYPE, FsUploadPayload { path })?,
                        FS_UPLOAD_READY_TYPE,
                        FILESYSTEM_DOWNLOAD_OFFER_TIMEOUT,
                    )
                    .await?;
                runtime
                    .send_file_offer_from_path(&device_id, source_path, Some(request_id))
                    .await
            }
            .await;

            match result {
                Ok(transfer) => {
                    let _ = runtime.emit_transfers();
                    runtime.update_remote_filesystem_upload(&upload_id, |upload| {
                        upload.session_id = Some(transfer.file_id);
                    });
                }
                Err(error) => runtime.fail_remote_filesystem_upload(&upload_id, error.to_string()),
            }
        });
        Ok(upload)
    }

    pub fn remote_filesystem_downloads(&self) -> Vec<RemoteFilesystemDownload> {
        let mut downloads = self
            .inner
            .state
            .lock_unpoisoned()
            .remote_filesystem_downloads
            .values()
            .cloned()
            .collect::<Vec<_>>();
        downloads.sort_by(|left, right| right.requested_at.cmp(&left.requested_at));
        downloads
    }

    pub fn remote_filesystem_uploads(&self) -> Vec<RemoteFilesystemUpload> {
        let mut uploads = self
            .inner
            .state
            .lock_unpoisoned()
            .remote_filesystem_uploads
            .values()
            .cloned()
            .collect::<Vec<_>>();
        uploads.sort_by(|left, right| right.requested_at.cmp(&left.requested_at));
        uploads
    }

    pub(super) async fn handle_filesystem_message(
        &self,
        from: &str,
        route: &str,
        envelope_id: Option<String>,
        message: BusinessEnvelope,
    ) {
        let Some(request_id) = envelope_id else {
            warn!(%from, message_type = %message.message_type, "ignored filesystem request without envelope id");
            return;
        };

        match message.message_type.as_str() {
            FS_ROOTS_TYPE => {
                if decode_request::<FsRootsPayload>(&message).is_err() {
                    self.send_filesystem_error(from, &request_id, FilesystemError::generic("invalid roots request")).await;
                    return;
                }
                let result = tokio::task::spawn_blocking(filesystem_roots)
                    .await
                    .unwrap_or_else(|error| Err(FilesystemError::generic(error.to_string())));
                match result {
                    Ok(payload) => self.send_filesystem_payload(from, &request_id, FS_ROOTS_RESULT_TYPE, payload).await,
                    Err(error) => self.send_filesystem_error(from, &request_id, error).await,
                }
            }
            FS_LIST_TYPE => {
                let request = match decode_request::<FsListPayload>(&message) {
                    Ok(request) => request,
                    Err(error) => {
                        self.send_filesystem_error(from, &request_id, error).await;
                        return;
                    }
                };
                let result = tokio::task::spawn_blocking(move || filesystem_list(request))
                    .await
                    .unwrap_or_else(|error| Err(FilesystemError::generic(error.to_string())));
                match result {
                    Ok(payload) => self.send_filesystem_payload(from, &request_id, FS_LIST_RESULT_TYPE, payload).await,
                    Err(error) => self.send_filesystem_error(from, &request_id, error).await,
                }
            }
            FS_STAT_TYPE => {
                let request = match decode_request::<FsStatPayload>(&message) {
                    Ok(request) => request,
                    Err(error) => {
                        self.send_filesystem_error(from, &request_id, error).await;
                        return;
                    }
                };
                let result = tokio::task::spawn_blocking(move || filesystem_stat(request))
                    .await
                    .unwrap_or_else(|error| Err(FilesystemError::generic(error.to_string())));
                match result {
                    Ok(payload) => self.send_filesystem_payload(from, &request_id, FS_STAT_RESULT_TYPE, payload).await,
                    Err(error) => self.send_filesystem_error(from, &request_id, error).await,
                }
            }
            FS_DOWNLOAD_TYPE => {
                let request = match decode_request::<FsDownloadPayload>(&message) {
                    Ok(request) => request,
                    Err(error) => {
                        self.send_filesystem_error(from, &request_id, error).await;
                        return;
                    }
                };
                let result = tokio::task::spawn_blocking(move || filesystem_download_path(request))
                    .await
                    .unwrap_or_else(|error| Err(FilesystemError::generic(error.to_string())));
                match result {
                    Ok(path) => match self
                        .send_file_offer_from_path(from, path.clone(), Some(request_id.clone()))
                        .await
                    {
                        Ok(_) => {
                            let _ = self.emit_transfers();
                            info!(%from, request_id = %request_id, "remote filesystem file offer sent");
                        }
                        Err(error) => {
                            warn!(%from, %error, path = %path.display(), "filesystem download offer failed");
                            self.send_filesystem_error(
                                from,
                                &request_id,
                                FilesystemError {
                                    reason: "io_error",
                                    message: error.to_string(),
                                },
                            )
                            .await;
                        }
                    },
                    Err(error) => self.send_filesystem_error(from, &request_id, error).await,
                }
            }
            FS_UPLOAD_TYPE => {
                let request = match decode_request::<FsUploadPayload>(&message) {
                    Ok(request) => request,
                    Err(error) => {
                        self.send_filesystem_error(from, &request_id, error).await;
                        return;
                    }
                };
                let result = tokio::task::spawn_blocking(move || prepare_filesystem_upload(&request.path))
                    .await
                    .unwrap_or_else(|error| Err(FilesystemError::generic(error.to_string())));
                match result {
                    Ok(destination) => {
                        if !self.reserve_filesystem_upload(from, route, &request_id, destination) {
                            self.send_filesystem_error(
                                from,
                                &request_id,
                                FilesystemError { reason: "already_exists", message: "upload destination is already reserved".to_string() },
                            ).await;
                            return;
                        }
                        let sent = match BusinessEnvelope::from_payload(
                            FS_UPLOAD_READY_TYPE,
                            FsUploadReadyPayload {},
                        ) {
                            Ok(message) => self
                                .send_business_message_with_correlation(
                                    from,
                                    message,
                                    Some(request_id.clone()),
                                )
                                .await
                                .map(|_| ()),
                            Err(error) => Err(AppError::from(error)),
                        };
                        match sent {
                            Ok(_) => self.expire_filesystem_upload(request_id),
                            Err(error) => {
                                self.remove_filesystem_upload(&request_id);
                                warn!(%from, %error, "filesystem upload-ready send failed");
                            }
                        }
                    }
                    Err(error) => self.send_filesystem_error(from, &request_id, error).await,
                }
            }
            _ => {}
        }
    }

    async fn send_filesystem_payload<T: Serialize>(
        &self,
        device_id: &str,
        request_id: &str,
        message_type: &str,
        payload: T,
    ) {
        let result: AppResult<()> = async {
            let response = BusinessEnvelope::from_payload(message_type, payload)?;
            self.send_business_message_with_correlation(
                device_id,
                response,
                Some(request_id.to_string()),
            )
            .await?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            warn!(%device_id, %error, message_type, "filesystem response send failed");
        }
    }

    async fn send_filesystem_error(
        &self,
        device_id: &str,
        request_id: &str,
        error: FilesystemError,
    ) {
        debug!(%device_id, reason = error.reason, message = %error.message, "filesystem request failed");
        self.send_filesystem_payload(
            device_id,
            request_id,
            FS_ERROR_TYPE,
            FsErrorPayload {
                reason: error.reason.to_string(),
                message: error.message,
                details: None,
            },
        )
        .await;
    }

    pub(super) fn associate_remote_filesystem_file_offer(
        &self,
        device_id: &str,
        correlation_id: Option<&str>,
        session_id: &str,
    ) -> Option<String> {
        let request_id = correlation_id?;
        let matched = {
            let mut state = self.inner.state.lock_unpoisoned();
            let Some(download) = state.remote_filesystem_downloads.get_mut(request_id) else {
                return None;
            };
            if download.device_id != device_id || download.session_id.is_some() || download.error.is_some() {
                return None;
            }
            download.session_id = Some(session_id.to_string());
            true
        };
        if matched {
            self.emit_remote_filesystem_downloads();
            Some(request_id.to_string())
        } else {
            None
        }
    }

    pub(super) fn fail_remote_filesystem_download(&self, request_id: &str, message: String) {
        let updated = {
            let mut state = self.inner.state.lock_unpoisoned();
            let Some(download) = state.remote_filesystem_downloads.get_mut(request_id) else {
                return;
            };
            if download.error.is_some() {
                return;
            }
            download.error = Some(message);
            true
        };
        if updated {
            self.emit_remote_filesystem_downloads();
        }
    }

    pub(super) fn complete_filesystem_request(
        &self,
        device_id: &str,
        correlation_id: Option<&str>,
        message: &BusinessEnvelope,
    ) {
        let Some(request_id) = correlation_id else {
            debug!(%device_id, message_type = %message.message_type, "ignored filesystem response without correlation id");
            return;
        };
        let pending = {
            let mut state = self.inner.state.lock_unpoisoned();
            let Some(pending) = state.pending_filesystem_requests.get(request_id) else {
                debug!(%device_id, %request_id, message_type = %message.message_type, "ignored filesystem response without pending request");
                return;
            };
            if pending.device_id != device_id
                || (message.message_type != FS_ERROR_TYPE
                    && message.message_type != pending.expected_response_type)
            {
                warn!(%device_id, %request_id, message_type = %message.message_type, expected_response_type = pending.expected_response_type, "ignored mismatched filesystem response");
                return;
            }
            state.pending_filesystem_requests.remove(request_id)
        };
        let Some(pending) = pending else {
            return;
        };
        let result = if message.message_type == FS_ERROR_TYPE {
            let error = serde_json::from_value::<FsErrorPayload>(message.payload.clone())
                .map(filesystem_error_token)
                .unwrap_or_else(|_| "colink:fs.io_error.v1".to_string());
            Err(AppError::message(error))
        } else {
            Ok(message.clone())
        };
        debug!(%device_id, %request_id, message_type = %message.message_type, "completed remote filesystem request");
        let _ = pending.sender.send(result);
    }

    pub(super) fn complete_remote_filesystem_download_error(
        &self,
        device_id: &str,
        correlation_id: Option<&str>,
        message: &BusinessEnvelope,
    ) {
        let Some(request_id) = correlation_id else {
            return;
        };
        let error = serde_json::from_value::<FsErrorPayload>(message.payload.clone())
            .map(filesystem_error_token)
            .unwrap_or_else(|_| "colink:fs.io_error.v1".to_string());
        let matched = self
            .inner
            .state
            .lock_unpoisoned()
            .remote_filesystem_downloads
            .get(request_id)
            .is_some_and(|download| download.device_id == device_id);
        if matched {
            self.fail_remote_filesystem_download(request_id, error);
        }
    }

    fn require_remote_filesystem_support(&self, device_id: &str) -> AppResult<()> {
        if self
            .peer_business_version(device_id)
            .is_some_and(|version| !crate::protocol::supports_business_protocol_at_least(&version, 1, 4, 0))
        {
            return Err(AppError::message(
                REMOTE_FILESYSTEM_UNSUPPORTED_ERROR,
            ));
        }
        Ok(())
    }

    fn require_remote_filesystem_upload_support(&self, device_id: &str) -> AppResult<()> {
        if !self
            .peer_business_version(device_id)
            .is_some_and(|version| crate::protocol::supports_business_protocol_at_least(&version, 1, 13, 0))
        {
            return Err(AppError::message(REMOTE_FILESYSTEM_UPLOAD_UNSUPPORTED_ERROR));
        }
        Ok(())
    }

    async fn request_remote_filesystem(
        &self,
        device_id: &str,
        request: BusinessEnvelope,
        expected_response_type: &'static str,
    ) -> AppResult<BusinessEnvelope> {
        self.request_remote_filesystem_with_id(
            device_id,
            request,
            expected_response_type,
            FILESYSTEM_REQUEST_TIMEOUT,
        ).await.map(|(_, response)| response)
    }

    async fn request_remote_filesystem_with_id(
        &self,
        device_id: &str,
        request: BusinessEnvelope,
        expected_response_type: &'static str,
        request_timeout: std::time::Duration,
    ) -> AppResult<(String, BusinessEnvelope)> {
        let request_id = Uuid::new_v4().to_string();
        debug!(%device_id, %request_id, request_type = %request.message_type, %expected_response_type, "sending remote filesystem request");
        let (sender, receiver) = oneshot::channel();
        self.inner.state.lock_unpoisoned().pending_filesystem_requests.insert(
            request_id.clone(),
            PendingFilesystemRequest {
                device_id: device_id.to_string(),
                expected_response_type,
                sender,
            },
        );

        if let Err(error) = self
            .send_business_message_with_envelope_id(device_id, request, request_id.clone())
            .await
        {
            self.inner
                .state
                .lock_unpoisoned()
                .pending_filesystem_requests
                .remove(&request_id);
            return Err(error);
        }

        let result = match timeout(request_timeout, receiver).await {
            Ok(Ok(result)) => result.map(|response| (request_id.clone(), response)),
            Ok(Err(_)) => Err(AppError::message("Remote filesystem request ended unexpectedly")),
            Err(_) => Err(AppError::message("Remote device did not respond in time")),
        };
        if let Err(error) = &result {
            warn!(%device_id, %request_id, error = %error, "remote filesystem request failed");
        }
        self.inner
            .state
            .lock_unpoisoned()
            .pending_filesystem_requests
            .remove(&request_id);
        result
    }

    fn remember_remote_filesystem_download(&self, download: RemoteFilesystemDownload) {
        {
            let mut state = self.inner.state.lock_unpoisoned();
            state
                .remote_filesystem_downloads
                .insert(download.request_id.clone(), download);
            if state.remote_filesystem_downloads.len() > 100 {
                let mut stale = state
                    .remote_filesystem_downloads
                    .values()
                    .map(|item| (item.request_id.clone(), item.requested_at))
                    .collect::<Vec<_>>();
                stale.sort_by(|left, right| right.1.cmp(&left.1));
                for (request_id, _) in stale.into_iter().skip(100) {
                    state.remote_filesystem_downloads.remove(&request_id);
                }
            }
        }
        self.emit_remote_filesystem_downloads();
    }

    fn remove_remote_filesystem_download(&self, request_id: &str) {
        if self
            .inner
            .state
            .lock_unpoisoned()
            .remote_filesystem_downloads
            .remove(request_id)
            .is_some()
        {
            self.emit_remote_filesystem_downloads();
        }
    }

    fn expire_remote_filesystem_download(&self, request_id: &str) {
        let waiting = self
            .inner
            .state
            .lock_unpoisoned()
            .remote_filesystem_downloads
            .get(request_id)
            .is_some_and(|download| download.session_id.is_none() && download.error.is_none());
        if waiting {
            self.fail_remote_filesystem_download(
                request_id,
                "Remote device did not start the download".to_string(),
            );
        }
    }

    fn emit_remote_filesystem_downloads(&self) {
        let _ = self
            .inner
            .app
            .emit(REMOTE_FILESYSTEM_DOWNLOADS_UPDATED_EVENT, self.remote_filesystem_downloads());
    }

    fn remember_remote_filesystem_upload(&self, upload: RemoteFilesystemUpload) {
        {
            let mut state = self.inner.state.lock_unpoisoned();
            state
                .remote_filesystem_uploads
                .insert(upload.request_id.clone(), upload);
            trim_remote_filesystem_uploads(&mut state.remote_filesystem_uploads);
        }
        self.emit_remote_filesystem_uploads();
    }

    fn update_remote_filesystem_upload(
        &self,
        request_id: &str,
        update: impl FnOnce(&mut RemoteFilesystemUpload),
    ) {
        let updated = {
            let mut state = self.inner.state.lock_unpoisoned();
            let Some(upload) = state.remote_filesystem_uploads.get_mut(request_id) else {
                return;
            };
            update(upload);
            true
        };
        if updated {
            self.emit_remote_filesystem_uploads();
        }
    }

    fn fail_remote_filesystem_upload(&self, request_id: &str, error: String) {
        self.update_remote_filesystem_upload(request_id, |upload| {
            if upload.error.is_none() {
                upload.error = Some(error);
            }
        });
    }

    fn emit_remote_filesystem_uploads(&self) {
        let _ = self
            .inner
            .app
            .emit(REMOTE_FILESYSTEM_UPLOADS_UPDATED_EVENT, self.remote_filesystem_uploads());
    }

    fn reserve_filesystem_upload(
        &self,
        device_id: &str,
        route: &str,
        request_id: &str,
        destination: FilesystemUploadDestination,
    ) -> bool {
        let mut state = self.inner.state.lock_unpoisoned();
        if state
            .pending_filesystem_uploads
            .values()
            .any(|pending| pending.destination.target == destination.target)
        {
            return false;
        }
        state.pending_filesystem_uploads.insert(
            request_id.to_string(),
            PendingFilesystemUpload {
                device_id: device_id.to_string(),
                route: route.to_string(),
                destination,
            },
        );
        true
    }

    fn expire_filesystem_upload(&self, request_id: String) {
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            sleep(FILESYSTEM_DOWNLOAD_OFFER_TIMEOUT).await;
            runtime.remove_filesystem_upload(&request_id);
        });
    }

    fn remove_filesystem_upload(&self, request_id: &str) {
        self.inner
            .state
            .lock_unpoisoned()
            .pending_filesystem_uploads
            .remove(request_id);
    }

    pub(super) fn consume_filesystem_upload(
        &self,
        device_id: &str,
        correlation_id: Option<&str>,
    ) -> Option<FilesystemUploadDestination> {
        let request_id = correlation_id?;
        let mut state = self.inner.state.lock_unpoisoned();
        let pending = state.pending_filesystem_uploads.get(request_id)?;
        if pending.device_id != device_id {
            return None;
        }
        state
            .pending_filesystem_uploads
            .remove(request_id)
            .map(|pending| pending.destination)
    }

    pub(super) fn clear_filesystem_uploads(&self, device_id: &str, route: &str) {
        self.inner.state.lock_unpoisoned().pending_filesystem_uploads.retain(|_, pending| {
            pending.device_id != device_id || pending.route != route
        });
    }

    pub(super) fn clear_filesystem_uploads_for_route(&self, route: &str) {
        self.inner
            .state
            .lock_unpoisoned()
            .pending_filesystem_uploads
            .retain(|_, pending| pending.route != route);
    }
}

fn decode_request<T: DeserializeOwned>(message: &BusinessEnvelope) -> Result<T, FilesystemError> {
    serde_json::from_value(message.payload.clone())
        .map_err(|_| FilesystemError::generic("invalid filesystem request payload"))
}

fn filesystem_roots() -> Result<FsRootsResultPayload, FilesystemError> {
    #[cfg(windows)]
    let roots = (b'A'..=b'Z')
        .filter_map(|letter| {
            let path = format!("{}:\\", letter as char);
            Path::new(&path).is_dir().then_some(FsRootEntry {
                label: volume_label(&path).or_else(|| Some(format!("{}:", letter as char))),
                path,
                total_bytes: None,
                free_bytes: None,
            })
        })
        .collect();

    #[cfg(not(windows))]
    let roots = vec![FsRootEntry {
        path: "/".to_string(),
        label: Some("/".to_string()),
        total_bytes: None,
        free_bytes: None,
    }];

    Ok(FsRootsResultPayload { roots })
}

#[cfg(windows)]
fn volume_label(root_path: &str) -> Option<String> {
    const VOLUME_LABEL_BUFFER_LEN: usize = 261;

    let root_path = root_path
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut label = [0_u16; VOLUME_LABEL_BUFFER_LEN];
    unsafe {
        GetVolumeInformationW(
            PCWSTR(root_path.as_ptr()),
            Some(&mut label),
            None,
            None,
            None,
            None,
        )
        .ok()?;
    }
    volume_label_from_utf16(&label)
}

#[cfg(windows)]
fn volume_label_from_utf16(label: &[u16]) -> Option<String> {
    let length = label.iter().position(|value| *value == 0).unwrap_or(label.len());
    let label = String::from_utf16_lossy(&label[..length]).trim().to_string();
    (!label.is_empty()).then_some(label)
}

fn filesystem_list(request: FsListPayload) -> Result<FsListResultPayload, FilesystemError> {
    let path = absolute_path(&request.path)?;
    let metadata = fs::metadata(&path).map_err(FilesystemError::from_io)?;
    if !metadata.is_dir() {
        return Err(FilesystemError {
            reason: "not_directory",
            message: "path is not a directory".to_string(),
        });
    }

    let mut entries = fs::read_dir(&path)
        .map_err(FilesystemError::from_io)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            filesystem_entry(name, entry.path()).ok()
        })
        .collect::<Vec<_>>();
    entries.sort_by(compare_filesystem_entries);

    let total = i64::try_from(entries.len()).unwrap_or(i64::MAX);
    let offset = request.offset.unwrap_or(0).clamp(0, total);
    let limit = request
        .limit
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT);
    let start = usize::try_from(offset).unwrap_or(entries.len());
    let entries = entries
        .into_iter()
        .skip(start)
        .take(usize::try_from(limit).unwrap_or(MAX_LIST_LIMIT as usize))
        .collect::<Vec<_>>();
    let has_more = offset.saturating_add(entries.len() as i64) < total;

    Ok(FsListResultPayload {
        path: request.path,
        entries,
        total,
        offset,
        has_more,
    })
}

fn compare_filesystem_entries(left: &FsEntry, right: &FsEntry) -> std::cmp::Ordering {
    let left_kind = (left.kind != "directory") as u8;
    let right_kind = (right.kind != "directory") as u8;
    left_kind
        .cmp(&right_kind)
        .then_with(|| right.modified.cmp(&left.modified))
        .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        .then_with(|| left.name.cmp(&right.name))
}

fn filesystem_stat(request: FsStatPayload) -> Result<FsStatResultPayload, FilesystemError> {
    let path = absolute_path(&request.path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(FsStatResultPayload {
                path: request.path,
                exists: false,
                kind: None,
                size: None,
                modified: None,
                created: None,
                readonly: None,
                hidden: None,
            });
        }
        Err(error) => return Err(FilesystemError::from_io(error)),
    };
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string();
    let entry = filesystem_entry_from_metadata(name, metadata);
    Ok(FsStatResultPayload {
        path: request.path,
        exists: true,
        kind: Some(entry.kind),
        size: entry.size,
        modified: entry.modified,
        created: entry.created,
        readonly: Some(entry.readonly),
        hidden: Some(entry.hidden),
    })
}

fn filesystem_download_path(request: FsDownloadPayload) -> Result<PathBuf, FilesystemError> {
    let path = absolute_path(&request.path)?;
    let metadata = fs::metadata(&path).map_err(FilesystemError::from_io)?;
    if !metadata.is_file() {
        return Err(FilesystemError {
            reason: "not_file",
            message: "path is not a regular file".to_string(),
        });
    }
    Ok(path)
}

fn prepare_filesystem_upload(path: &str) -> Result<FilesystemUploadDestination, FilesystemError> {
    let target = absolute_path(path)?;
    let file_name = target
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| FilesystemError { reason: "invalid_path", message: "upload destination must be a file path".to_string() })?;
    let parent = target.parent().ok_or_else(|| FilesystemError {
        reason: "invalid_path",
        message: "upload destination must have a parent directory".to_string(),
    })?;
    let parent = validate_upload_parent(parent)?;
    let target = parent.join(file_name);
    require_absent_upload_target(&target)?;
    Ok(FilesystemUploadDestination { parent, target })
}

pub(super) fn create_filesystem_upload_temp(
    destination: &FilesystemUploadDestination,
    expected_size: i64,
) -> Result<PathBuf, FilesystemError> {
    let parent = revalidate_upload_parent(destination)?;
    if expected_size < 0 || u64::try_from(expected_size).ok().is_none_or(|size| available_space(&parent) < size) {
        return Err(FilesystemError { reason: "io_error", message: "insufficient storage for upload".to_string() });
    }
    let path = parent.join(format!(".colink-{}.part", Uuid::new_v4()));
    fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(FilesystemError::from_io)?;
    Ok(path)
}

pub(super) fn commit_filesystem_upload(
    destination: &FilesystemUploadDestination,
    temp_path: &Path,
) -> Result<PathBuf, FilesystemError> {
    let parent = revalidate_upload_parent(destination)?;
    let target = parent.join(destination.target.file_name().unwrap_or_default());
    require_absent_upload_target(&target)?;
    fs::rename(temp_path, &target).map_err(FilesystemError::from_io)?;
    Ok(target)
}

fn validate_upload_parent(parent: &Path) -> Result<PathBuf, FilesystemError> {
    let metadata = fs::symlink_metadata(parent).map_err(FilesystemError::from_io)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || path_contains_link_or_reparse_point(parent)? {
        return Err(FilesystemError { reason: "invalid_path", message: "upload directory must not contain links or reparse points".to_string() });
    }
    fs::canonicalize(parent).map_err(FilesystemError::from_io)
}

fn revalidate_upload_parent(destination: &FilesystemUploadDestination) -> Result<PathBuf, FilesystemError> {
    let parent = validate_upload_parent(&destination.parent)?;
    if parent != destination.parent {
        return Err(FilesystemError { reason: "invalid_path", message: "upload directory changed after authorization".to_string() });
    }
    Ok(parent)
}

fn require_absent_upload_target(target: &Path) -> Result<(), FilesystemError> {
    match fs::symlink_metadata(target) {
        Ok(_) => Err(FilesystemError { reason: "already_exists", message: "upload destination already exists".to_string() }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(FilesystemError::from_io(error)),
    }
}

// Non-general filesystem errors remain stable protocol tokens until the UI maps them
// to its own localized text. This avoids exposing host paths or OS error details.
fn filesystem_error_token(payload: FsErrorPayload) -> String {
    if matches!(payload.reason.as_str(), "general" | "generic") {
        payload.message
    } else {
        format!("colink:fs.{}.v1", payload.reason)
    }
}

fn trim_remote_filesystem_uploads(
    uploads: &mut std::collections::HashMap<String, RemoteFilesystemUpload>,
) {
    if uploads.len() <= 100 {
        return;
    }
    let mut stale = uploads
        .values()
        .map(|upload| (upload.request_id.clone(), upload.requested_at))
        .collect::<Vec<_>>();
    stale.sort_by(|left, right| right.1.cmp(&left.1));
    for (request_id, _) in stale.into_iter().skip(100) {
        uploads.remove(&request_id);
    }
}

fn available_space(path: &Path) -> u64 {
    fs2::available_space(path).unwrap_or_default()
}

fn path_contains_link_or_reparse_point(path: &Path) -> Result<bool, FilesystemError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        let metadata = fs::symlink_metadata(&current).map_err(FilesystemError::from_io)?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &Metadata) -> bool { false }

fn absolute_path(raw_path: &str) -> Result<PathBuf, FilesystemError> {
    let path = PathBuf::from(raw_path);
    if !path.is_absolute() {
        return Err(FilesystemError::generic("path must be absolute"));
    }
    Ok(path)
}

fn filesystem_entry(name: String, path: PathBuf) -> Result<FsEntry, FilesystemError> {
    let metadata = fs::symlink_metadata(path).map_err(FilesystemError::from_io)?;
    Ok(filesystem_entry_from_metadata(name, metadata))
}

fn filesystem_entry_from_metadata(name: String, metadata: Metadata) -> FsEntry {
    let kind = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    };
    FsEntry {
        hidden: is_hidden(&name, &metadata),
        name,
        kind: kind.to_string(),
        size: metadata.is_file().then(|| i64::try_from(metadata.len()).ok()).flatten(),
        modified: system_time_millis(metadata.modified()),
        created: system_time_millis(metadata.created()),
        readonly: metadata.permissions().readonly(),
    }
}

fn system_time_millis(value: io::Result<SystemTime>) -> Option<i64> {
    let millis = value.ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(millis).ok()
}

#[cfg(windows)]
fn is_hidden(_name: &str, metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes() & 0x2 != 0
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _metadata: &Metadata) -> bool {
    name.starts_with('.')
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use uuid::Uuid;

    use super::{
        commit_filesystem_upload, compare_filesystem_entries, create_filesystem_upload_temp,
        filesystem_error_token, filesystem_list, prepare_filesystem_upload, FsEntry, FsErrorPayload,
        FsListPayload,
    };
    #[cfg(windows)]
    use super::volume_label_from_utf16;

    #[cfg(windows)]
    #[test]
    fn decodes_nonempty_utf16_volume_labels() {
        let encoded = |label: &str| {
            label
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>()
        };

        assert_eq!(volume_label_from_utf16(&encoded("Data")), Some("Data".to_string()));
        assert_eq!(volume_label_from_utf16(&encoded("系统")), Some("系统".to_string()));
        assert_eq!(volume_label_from_utf16(&encoded("   ")), None);
    }

    #[test]
    fn sorts_directories_and_files_by_modified_time_descending() {
        let entry = |name: &str, kind: &str, modified: Option<i64>| FsEntry {
            name: name.to_string(),
            kind: kind.to_string(),
            size: None,
            modified,
            created: None,
            readonly: false,
            hidden: false,
        };
        let mut entries = vec![
            entry("folder-old", "directory", Some(10)),
            entry("folder-alpha", "directory", Some(10)),
            entry("file-unknown", "file", None),
            entry("file-alpha", "file", None),
            entry("file-new", "file", Some(40)),
            entry("folder-new", "directory", Some(30)),
            entry("file-old", "file", Some(20)),
        ];

        entries.sort_by(compare_filesystem_entries);

        assert_eq!(
            entries.into_iter().map(|entry| entry.name).collect::<Vec<_>>(),
            [
                "folder-new",
                "folder-alpha",
                "folder-old",
                "file-new",
                "file-old",
                "file-alpha",
                "file-unknown",
            ],
        );
    }

    #[test]
    fn lists_directories_first_with_stable_pagination() {
        let root = std::env::temp_dir().join(format!("colink-filesystem-test-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("zebra")).unwrap();
        fs::write(root.join("alpha.txt"), b"alpha").unwrap();
        fs::write(root.join("bravo.txt"), b"bravo").unwrap();

        let first_page = filesystem_list(FsListPayload {
            path: root.to_string_lossy().to_string(),
            offset: Some(0),
            limit: Some(2),
        })
        .unwrap();
        let second_page = filesystem_list(FsListPayload {
            path: root.to_string_lossy().to_string(),
            offset: Some(2),
            limit: Some(2),
        })
        .unwrap();

        assert_eq!(first_page.total, 3);
        assert!(first_page.has_more);
        assert_eq!(first_page.entries[0].name, "zebra");
        assert_eq!(first_page.entries[0].kind, "directory");
        assert!(!second_page.has_more);
        let mut names = first_page
            .entries
            .into_iter()
            .chain(second_page.entries)
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, ["alpha.txt", "bravo.txt", "zebra"]);

        fs::remove_dir_all(PathBuf::from(root)).unwrap();
    }

    #[test]
    fn rejects_relative_paths() {
        let error = filesystem_list(FsListPayload {
            path: "relative".to_string(),
            offset: None,
            limit: None,
        })
        .unwrap_err();

        assert_eq!(error.reason, "generic");
    }

    #[test]
    fn filesystem_errors_prefer_structured_codes() {
        assert_eq!(
            filesystem_error_token(FsErrorPayload {
                reason: "permission_denied".to_string(),
                message: "internal path must not be exposed".to_string(),
                details: None,
            }),
            "colink:fs.permission_denied.v1",
        );
        assert_eq!(
            filesystem_error_token(FsErrorPayload {
                reason: "general".to_string(),
                message: "remote filesystem request failed".to_string(),
                details: None,
            }),
            "remote filesystem request failed",
        );
    }

    #[test]
    fn commits_authorized_upload_without_overwriting() {
        let root = std::env::temp_dir().join(format!("colink-upload-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("report.txt");
        let destination = prepare_filesystem_upload(&target.to_string_lossy()).unwrap();
        let temp = create_filesystem_upload_temp(&destination, 5).unwrap();
        fs::write(&temp, b"hello").unwrap();
        let committed = commit_filesystem_upload(&destination, &temp).unwrap();
        assert_eq!(fs::read(&committed).unwrap(), b"hello");
        assert!(prepare_filesystem_upload(&target.to_string_lossy()).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn upload_commit_preserves_a_file_created_after_authorization() {
        let root = std::env::temp_dir().join(format!("colink-upload-race-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("report.txt");
        let destination = prepare_filesystem_upload(&target.to_string_lossy()).unwrap();
        let temp = create_filesystem_upload_temp(&destination, 5).unwrap();
        fs::write(&temp, b"hello").unwrap();
        fs::write(&target, b"existing").unwrap();

        assert!(commit_filesystem_upload(&destination, &temp).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"existing");
        fs::remove_file(temp).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
