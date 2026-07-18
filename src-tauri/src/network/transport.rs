use crate::{
    error::{AppError, AppResult},
    i18n::{self, TextKey},
    network::{cloud::CloudConnectionManager, lan::LanManager},
    protocol::BusinessEnvelope,
    store::db::Database,
};

#[derive(Clone)]
pub struct TransportManager {
    database: Database,
    lan: LanManager,
    cloud: CloudConnectionManager,
}

impl TransportManager {
    pub fn new(database: Database, lan: LanManager, cloud: CloudConnectionManager) -> Self {
        Self {
            database,
            lan,
            cloud,
        }
    }

    pub async fn send(
        &self,
        device_id: &str,
        message: BusinessEnvelope,
        envelope_id: Option<String>,
        correlation_id: Option<String>,
    ) -> AppResult<String> {
        if self.lan.is_available(device_id) {
            match self
                .lan
                .send(
                    device_id,
                    message.clone(),
                    envelope_id.clone(),
                    correlation_id.clone(),
                )
                .await
            {
                Ok(()) => return Ok("lan".to_string()),
                Err(error) => {
                    tracing::warn!(%device_id, %error, "lan send failed; trying cloud fallback");
                }
            }
        }

        if self.cloud.is_connected() {
            self.cloud.ensure_business_compatible(device_id)?;
            self
                .cloud
                .send_relay(device_id, message, envelope_id, correlation_id)?;
            return Ok("cloud".to_string());
        }

        Err(AppError::message(
            self.user_text(TextKey::DeviceNotConnected),
        ))
    }

    #[allow(dead_code)]
    pub async fn send_lan_only(
        &self,
        device_id: &str,
        message: BusinessEnvelope,
    ) -> AppResult<String> {
        if self.lan.is_available(device_id) {
            self.lan.send(device_id, message, None, None).await?;
            return Ok("lan".to_string());
        }

        Err(AppError::message(
            self.user_text(TextKey::DeviceNotConnected),
        ))
    }

    pub fn broadcast_cloud(
        &self,
        message: BusinessEnvelope,
        correlation_id: Option<String>,
    ) -> AppResult<String> {
        if self.cloud.is_connected() {
            self.cloud.ensure_known_business_versions_compatible()?;
            self.cloud.send_broadcast(message, correlation_id)?;
            return Ok("cloud".to_string());
        }

        Err(AppError::message(
            self.user_text(TextKey::DeviceNotConnected),
        ))
    }

    fn user_text(&self, key: TextKey) -> String {
        let language = self
            .database
            .load_settings()
            .ok()
            .flatten()
            .map(|settings| settings.language)
            .unwrap_or_else(i18n::default_language_code);
        i18n::text(&language, key).to_string()
    }
}
