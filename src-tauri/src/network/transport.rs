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

    pub async fn send(&self, device_id: &str, message: BusinessEnvelope) -> AppResult<String> {
        if self.lan.is_available(device_id) {
            match self.lan.send(device_id, message.clone()).await {
                Ok(()) => return Ok("lan".to_string()),
                Err(error) => {
                    tracing::warn!(%device_id, %error, "lan send failed; trying cloud fallback");
                }
            }
        }

        if self.cloud.is_connected() {
            self.cloud.send_relay(device_id, message)?;
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
