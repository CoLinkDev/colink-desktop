use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;

use crate::error::AppResult;

pub struct GeneratedKeyPair {
    pub public_key: String,
    pub private_key: String,
}

pub fn generate_key_pair() -> AppResult<GeneratedKeyPair> {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    Ok(GeneratedKeyPair {
        public_key: STANDARD.encode(verifying_key.to_bytes()),
        private_key: STANDARD.encode(signing_key.to_bytes()),
    })
}
