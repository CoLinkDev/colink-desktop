use crate::error::{AppError, AppResult};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;

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

pub fn sign_payload(private_key: &str, payload: &[u8]) -> AppResult<String> {
    let bytes = STANDARD.decode(private_key)?;
    let signing_key = SigningKey::from_bytes(
        &bytes
            .try_into()
            .map_err(|_| AppError::Crypto("private key length invalid".to_string()))?,
    );
    let signature = signing_key.sign(payload);
    Ok(STANDARD.encode(signature.to_bytes()))
}

pub fn verify_signature(public_key: &str, payload: &[u8], signature: &str) -> AppResult<bool> {
    let public_key_bytes = STANDARD.decode(public_key)?;
    let signature_bytes = STANDARD.decode(signature)?;

    let verifying_key = VerifyingKey::from_bytes(
        &public_key_bytes
            .try_into()
            .map_err(|_| AppError::Crypto("public key length invalid".to_string()))?,
    )
    .map_err(|error| AppError::Crypto(error.to_string()))?;

    let signature = Signature::from_bytes(
        &signature_bytes
            .try_into()
            .map_err(|_| AppError::Crypto("signature length invalid".to_string()))?,
    );

    Ok(verifying_key.verify(payload, &signature).is_ok())
}

#[cfg(test)]
mod tests {
    use super::{generate_key_pair, sign_payload, verify_signature};

    #[test]
    fn signs_and_verifies_payload() {
        let pair = generate_key_pair().expect("key pair");
        let payload = b"colink-lan-proof";
        let signature = sign_payload(&pair.private_key, payload).expect("signature");
        let valid = verify_signature(&pair.public_key, payload, &signature).expect("verify");
        let invalid = verify_signature(&pair.public_key, b"other", &signature).expect("verify");

        assert!(valid);
        assert!(!invalid);
    }
}
