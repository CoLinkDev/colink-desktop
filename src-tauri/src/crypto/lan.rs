use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce as AesNonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{ChaCha20Poly1305, Nonce as ChaChaNonce};
use curve25519_dalek::edwards::CompressedEdwardsY;
use hkdf::Hkdf;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256, Sha512};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

use crate::{
    error::{AppError, AppResult},
    protocol::{BusinessEnvelope, EncryptedBusinessPayload},
};

pub const AES_256_GCM_SUITE: &str = "x25519-aes-256-gcm";
pub const CHACHA20_POLY1305_SUITE: &str = "x25519-chacha20-poly1305";

pub struct LanEphemeralKeyPair {
    pub public_key: String,
    private_key: StaticSecret,
}

impl LanEphemeralKeyPair {
    pub fn generate() -> Self {
        let private_key = StaticSecret::random_from_rng(OsRng);
        let public_key = X25519PublicKey::from(&private_key);
        Self {
            public_key: STANDARD.encode(public_key.as_bytes()),
            private_key,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherSuite {
    Aes256Gcm,
    ChaCha20Poly1305,
}

impl CipherSuite {
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            AES_256_GCM_SUITE => Some(Self::Aes256Gcm),
            CHACHA20_POLY1305_SUITE => Some(Self::ChaCha20Poly1305),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Aes256Gcm => AES_256_GCM_SUITE,
            Self::ChaCha20Poly1305 => CHACHA20_POLY1305_SUITE,
        }
    }
}

pub fn supported_suites() -> Vec<String> {
    vec![
        AES_256_GCM_SUITE.to_string(),
        CHACHA20_POLY1305_SUITE.to_string(),
    ]
}

pub fn choose_suite(
    local_supported: &[String],
    peer_supported: &[String],
    local_is_initiator: bool,
) -> Option<CipherSuite> {
    let ordered = if local_is_initiator {
        local_supported
    } else {
        peer_supported
    };
    let other = if local_is_initiator {
        peer_supported
    } else {
        local_supported
    };

    ordered
        .iter()
        .find(|suite| other.iter().any(|item| item == *suite))
        .and_then(|suite| CipherSuite::from_wire(suite))
}

pub struct LanSessionCrypto {
    suite: CipherSuite,
    key: [u8; 32],
    outbound_role: u8,
    counter: u64,
}

impl LanSessionCrypto {
    pub fn new(
        suite: CipherSuite,
        private_key: &str,
        peer_public_key: &str,
        local_is_initiator: bool,
    ) -> AppResult<Self> {
        Ok(Self {
            suite,
            key: derive_session_key(private_key, peer_public_key)?,
            outbound_role: if local_is_initiator { 0 } else { 1 },
            counter: 0,
        })
    }

    pub fn new_with_ephemeral_keys(
        suite: CipherSuite,
        local_ephemeral: &LanEphemeralKeyPair,
        peer_ephemeral_public_key: &str,
        local_device_id: &str,
        peer_device_id: &str,
        protocol_version: &str,
        local_is_initiator: bool,
    ) -> AppResult<Self> {
        Ok(Self {
            suite,
            key: derive_session_key_from_ephemeral(
                suite,
                local_ephemeral,
                peer_ephemeral_public_key,
                local_device_id,
                peer_device_id,
                protocol_version,
            )?,
            outbound_role: if local_is_initiator { 0 } else { 1 },
            counter: 0,
        })
    }

    pub fn encrypt(&mut self, message: &BusinessEnvelope) -> AppResult<EncryptedBusinessPayload> {
        let plaintext = serde_json::to_vec(message)?;
        let nonce = self.next_nonce();
        let ciphertext = match self.suite {
            CipherSuite::Aes256Gcm => {
                let cipher = Aes256Gcm::new_from_slice(&self.key)
                    .map_err(|error| AppError::Crypto(error.to_string()))?;
                cipher
                    .encrypt(AesNonce::from_slice(&nonce), plaintext.as_ref())
                    .map_err(|error| AppError::Crypto(error.to_string()))?
            }
            CipherSuite::ChaCha20Poly1305 => {
                let cipher = ChaCha20Poly1305::new_from_slice(&self.key)
                    .map_err(|error| AppError::Crypto(error.to_string()))?;
                cipher
                    .encrypt(ChaChaNonce::from_slice(&nonce), plaintext.as_ref())
                    .map_err(|error| AppError::Crypto(error.to_string()))?
            }
        };

        Ok(EncryptedBusinessPayload {
            ciphertext: STANDARD.encode(ciphertext),
            nonce: STANDARD.encode(nonce),
        })
    }

    pub fn decrypt(&self, payload: &EncryptedBusinessPayload) -> AppResult<BusinessEnvelope> {
        let nonce = STANDARD.decode(&payload.nonce)?;
        let nonce: [u8; 12] = nonce
            .try_into()
            .map_err(|_| AppError::Crypto("LAN nonce length invalid".to_string()))?;
        let ciphertext = STANDARD.decode(&payload.ciphertext)?;
        let plaintext = match self.suite {
            CipherSuite::Aes256Gcm => {
                let cipher = Aes256Gcm::new_from_slice(&self.key)
                    .map_err(|error| AppError::Crypto(error.to_string()))?;
                cipher
                    .decrypt(AesNonce::from_slice(&nonce), ciphertext.as_ref())
                    .map_err(|error| AppError::Crypto(error.to_string()))?
            }
            CipherSuite::ChaCha20Poly1305 => {
                let cipher = ChaCha20Poly1305::new_from_slice(&self.key)
                    .map_err(|error| AppError::Crypto(error.to_string()))?;
                cipher
                    .decrypt(ChaChaNonce::from_slice(&nonce), ciphertext.as_ref())
                    .map_err(|error| AppError::Crypto(error.to_string()))?
            }
        };

        Ok(serde_json::from_slice(&plaintext)?)
    }

    fn next_nonce(&mut self) -> [u8; 12] {
        let mut nonce = [0_u8; 12];
        nonce[0] = self.outbound_role;
        nonce[4..].copy_from_slice(&self.counter.to_be_bytes());
        self.counter = self.counter.wrapping_add(1);
        nonce
    }
}

pub fn pairing_code(
    public_key_a: &str,
    public_key_b: &str,
    request_nonce: &str,
    exchange_nonce: &str,
) -> String {
    let mut keys = [public_key_a, public_key_b];
    keys.sort_unstable();
    let canonical = format!(
        "domain=colink-lan-pairing-code\npublicKeyA={}\npublicKeyB={}\nnonceA={}\nnonceB={}",
        keys[0], keys[1], request_nonce, exchange_nonce
    );

    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    let value = u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ]) % 1_000_000;

    format!("{value:06}")
}

fn derive_session_key(private_key: &str, peer_public_key: &str) -> AppResult<[u8; 32]> {
    let private = ed25519_private_to_x25519(private_key)?;
    let public = ed25519_public_to_x25519(peer_public_key)?;
    let shared = private.diffie_hellman(&public);
    let hkdf = Hkdf::<Sha256>::new(Some(b"colink-lan-v1"), shared.as_bytes());
    let mut key = [0_u8; 32];
    hkdf.expand(b"encryption", &mut key)
        .map_err(|error| AppError::Crypto(error.to_string()))?;
    Ok(key)
}

fn derive_session_key_from_ephemeral(
    suite: CipherSuite,
    local_ephemeral: &LanEphemeralKeyPair,
    peer_ephemeral_public_key: &str,
    local_device_id: &str,
    peer_device_id: &str,
    protocol_version: &str,
) -> AppResult<[u8; 32]> {
    let peer_bytes = STANDARD.decode(peer_ephemeral_public_key)?;
    let peer_bytes: [u8; 32] = peer_bytes
        .try_into()
        .map_err(|_| AppError::Crypto("ephemeral public key length invalid".to_string()))?;
    let peer_public_key = X25519PublicKey::from(peer_bytes);
    let shared = local_ephemeral.private_key.diffie_hellman(&peer_public_key);

    let local_first = local_device_id <= peer_device_id;
    let (from, to, ephemeral_a, ephemeral_b) = if local_first {
        (
            local_device_id,
            peer_device_id,
            local_ephemeral.public_key.as_str(),
            peer_ephemeral_public_key,
        )
    } else {
        (
            peer_device_id,
            local_device_id,
            peer_ephemeral_public_key,
            local_ephemeral.public_key.as_str(),
        )
    };
    let suite = match suite {
        CipherSuite::Aes256Gcm => AES_256_GCM_SUITE,
        CipherSuite::ChaCha20Poly1305 => CHACHA20_POLY1305_SUITE,
    };
    let info = format!(
        "domain=colink-lan-session-key\nfrom={from}\nto={to}\nephemeralA={ephemeral_a}\nephemeralB={ephemeral_b}\nprotocolVersion={protocol_version}\nsuite={suite}"
    );
    let hkdf = Hkdf::<Sha256>::new(Some(b"colink-lan-v2"), shared.as_bytes());
    let mut key = [0_u8; 32];
    hkdf.expand(info.as_bytes(), &mut key)
        .map_err(|error| AppError::Crypto(error.to_string()))?;
    Ok(key)
}

fn ed25519_private_to_x25519(private_key: &str) -> AppResult<StaticSecret> {
    let seed = STANDARD.decode(private_key)?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| AppError::Crypto("private key length invalid".to_string()))?;
    let digest = Sha512::digest(seed);
    let mut scalar = [0_u8; 32];
    scalar.copy_from_slice(&digest[..32]);
    scalar[0] &= 248;
    scalar[31] &= 127;
    scalar[31] |= 64;
    Ok(StaticSecret::from(scalar))
}

fn ed25519_public_to_x25519(public_key: &str) -> AppResult<X25519PublicKey> {
    let bytes = STANDARD.decode(public_key)?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| AppError::Crypto("public key length invalid".to_string()))?;
    let point = CompressedEdwardsY(bytes)
        .decompress()
        .ok_or_else(|| AppError::Crypto("public key conversion failed".to_string()))?;
    Ok(X25519PublicKey::from(point.to_montgomery().to_bytes()))
}

#[cfg(test)]
mod tests {
    use super::{
        choose_suite, pairing_code, supported_suites, LanSessionCrypto, AES_256_GCM_SUITE,
    };
    use crate::{crypto::keys::generate_key_pair, protocol::BusinessEnvelope};

    #[test]
    fn derives_stable_pairing_code() {
        let code_a = pairing_code("b", "a", "nonce-a", "nonce-b");
        let code_b = pairing_code("a", "b", "nonce-a", "nonce-b");

        assert_eq!(code_a, code_b);
        assert_eq!(code_a.len(), 6);
        assert_eq!(code_a, "893018");
    }

    #[test]
    fn chooses_suite_by_initiator_order() {
        let local = vec![
            "x25519-chacha20-poly1305".to_string(),
            AES_256_GCM_SUITE.to_string(),
        ];
        let peer = supported_suites();

        assert_eq!(
            choose_suite(&local, &peer, true).map(|suite| suite.as_wire()),
            Some("x25519-chacha20-poly1305")
        );
        assert!(choose_suite(&["none".to_string()], &peer, true).is_none());
    }

    #[test]
    fn encrypts_and_decrypts_business_message() {
        let first = generate_key_pair().expect("first key");
        let second = generate_key_pair().expect("second key");
        let mut first_crypto = LanSessionCrypto::new(
            super::CipherSuite::Aes256Gcm,
            &first.private_key,
            &second.public_key,
            true,
        )
        .expect("first crypto");
        let second_crypto = LanSessionCrypto::new(
            super::CipherSuite::Aes256Gcm,
            &second.private_key,
            &first.public_key,
            false,
        )
        .expect("second crypto");
        let message =
            BusinessEnvelope::from_payload("message.v1.text", serde_json::json!({"text":"hi"}))
                .expect("message");

        let encrypted = first_crypto.encrypt(&message).expect("encrypt");
        let decrypted = second_crypto.decrypt(&encrypted).expect("decrypt");

        assert_eq!(decrypted.message_type, "message.v1.text");
    }
}
