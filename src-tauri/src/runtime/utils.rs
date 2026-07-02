use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use sanitize_filename::sanitize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

const DEFAULT_FILE_CHECKSUM_ALGORITHM: FileChecksumAlgorithm = FileChecksumAlgorithm::Blake3;
const FILE_HASH_BUFFER_SIZE: usize = 1_048_576;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileChecksumAlgorithm {
    Sha256,
    Blake3,
    None,
}

impl FileChecksumAlgorithm {
    fn parse(name: &str) -> AppResult<Self> {
        match name.to_ascii_lowercase().as_str() {
            "sha256" => Ok(Self::Sha256),
            "blake3" => Ok(Self::Blake3),
            "none" => Ok(Self::None),
            _ => Err(AppError::message(format!(
                "unsupported checksum algorithm: {name}"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Blake3 => "blake3",
            Self::None => "none",
        }
    }
}

pub(super) struct FileChecksumVerifier {
    expected: String,
    hasher: FileChecksumHasher,
}

impl FileChecksumVerifier {
    pub(super) fn new(checksum: &str) -> AppResult<Self> {
        let (algorithm, expected) = split_checksum(checksum)?;
        if algorithm == FileChecksumAlgorithm::None && expected != "none" {
            return Err(AppError::message("none checksum must use none:none"));
        }
        Ok(Self {
            expected: expected.to_string(),
            hasher: FileChecksumHasher::new(algorithm),
        })
    }

    pub(super) fn update(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
    }

    pub(super) fn verify(&self) -> bool {
        self.hasher
            .digest_hex()
            .eq_ignore_ascii_case(&self.expected)
    }
}

struct FileChecksumHasher {
    inner: FileChecksumHasherKind,
}

#[derive(Clone)]
enum FileChecksumHasherKind {
    Sha256(Sha256),
    Blake3(blake3::Hasher),
    None,
}

impl FileChecksumHasher {
    fn new(algorithm: FileChecksumAlgorithm) -> Self {
        let inner = match algorithm {
            FileChecksumAlgorithm::Sha256 => FileChecksumHasherKind::Sha256(Sha256::new()),
            FileChecksumAlgorithm::Blake3 => FileChecksumHasherKind::Blake3(blake3::Hasher::new()),
            FileChecksumAlgorithm::None => FileChecksumHasherKind::None,
        };
        Self { inner }
    }

    fn update(&mut self, bytes: &[u8]) {
        match &mut self.inner {
            FileChecksumHasherKind::Sha256(hasher) => hasher.update(bytes),
            FileChecksumHasherKind::Blake3(hasher) => {
                hasher.update(bytes);
            }
            FileChecksumHasherKind::None => {}
        }
    }

    fn digest_hex(&self) -> String {
        match self.inner.clone() {
            FileChecksumHasherKind::Sha256(hasher) => format!("{:x}", hasher.finalize()),
            FileChecksumHasherKind::Blake3(hasher) => hasher.finalize().to_hex().to_string(),
            FileChecksumHasherKind::None => "none".to_string(),
        }
    }
}

pub(super) fn build_file_checksum(path: &Path) -> AppResult<String> {
    build_file_checksum_with_algorithm(path, DEFAULT_FILE_CHECKSUM_ALGORITHM)
}

fn build_file_checksum_with_algorithm(
    path: &Path,
    algorithm: FileChecksumAlgorithm,
) -> AppResult<String> {
    let digest = hash_file_by_algorithm(path, algorithm)?;
    Ok(format!("{}:{digest}", algorithm.as_str()))
}

pub(super) fn unique_download_path(download_dir: &Path, file_name: &str) -> PathBuf {
    let safe_name = sanitize(file_name);
    let candidate = download_dir.join(&safe_name);
    if !candidate.exists() {
        return candidate;
    }

    let stem = Path::new(&safe_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let extension = Path::new(&safe_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();

    for index in 2..1000 {
        let name = format!("{stem} ({index}){extension}");
        let path = download_dir.join(name);
        if !path.exists() {
            return path;
        }
    }

    download_dir.join(format!("{}-{}", Uuid::new_v4(), safe_name))
}

fn split_checksum(checksum: &str) -> AppResult<(FileChecksumAlgorithm, &str)> {
    if let Some((algorithm, digest)) = checksum.split_once(':') {
        return Ok((FileChecksumAlgorithm::parse(algorithm)?, digest));
    }

    Err(AppError::message(
        "checksum must include an algorithm prefix",
    ))
}

fn hash_file_by_algorithm(path: &Path, algorithm: FileChecksumAlgorithm) -> AppResult<String> {
    if algorithm == FileChecksumAlgorithm::Blake3 {
        let mut hasher = blake3::Hasher::new();
        hasher.update_mmap_rayon(path)?;
        return Ok(hasher.finalize().to_hex().to_string());
    }

    let mut file = fs::File::open(path)?;
    let mut buffer = vec![0_u8; FILE_HASH_BUFFER_SIZE];

    match algorithm {
        FileChecksumAlgorithm::Sha256 => {
            let mut hasher = Sha256::new();
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(format!("{:x}", hasher.finalize()))
        }
        FileChecksumAlgorithm::None => Ok("none".to_string()),
        FileChecksumAlgorithm::Blake3 => unreachable!("blake3 uses the parallel file path"),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    use super::{
        build_file_checksum_with_algorithm, split_checksum, unique_download_path,
        FileChecksumAlgorithm, FileChecksumVerifier,
    };

    #[test]
    fn verifies_prefixed_sha256_checksum_incrementally() {
        let mut hasher = Sha256::new();
        hasher.update(b"hello ");
        hasher.update(b"world");
        let checksum = format!("sha256:{:x}", hasher.finalize());

        let mut verifier = FileChecksumVerifier::new(&checksum).expect("verifier");
        verifier.update(b"hello ");
        verifier.update(b"world");

        assert!(verifier.verify());
    }

    #[test]
    fn rejects_checksum_without_supported_algorithm_prefix() {
        assert!(split_checksum("abc123").is_err());
        assert!(FileChecksumVerifier::new("md5:abc123").is_err());
    }

    #[test]
    fn verifies_none_checksum_without_hashing_content() {
        let mut verifier = FileChecksumVerifier::new("none:none").expect("verifier");
        verifier.update(b"payload");

        assert!(verifier.verify());
    }

    #[test]
    fn rejects_malformed_none_checksum() {
        assert!(FileChecksumVerifier::new("none:abc123").is_err());
    }

    #[test]
    fn builds_blake3_file_checksum_with_prefix() {
        let path = std::env::temp_dir().join(format!("colink-checksum-{}.bin", Uuid::new_v4()));
        fs::write(&path, b"payload").expect("write file");

        let checksum =
            build_file_checksum_with_algorithm(&path, FileChecksumAlgorithm::Blake3)
                .expect("checksum");

        assert!(checksum.starts_with("blake3:"));
        let mut verifier = FileChecksumVerifier::new(&checksum).expect("verifier");
        verifier.update(b"payload");
        assert!(verifier.verify());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn unique_download_path_uses_next_available_numbered_name() {
        let dir = std::env::temp_dir().join(format!("colink-download-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create dir");
        fs::File::create(dir.join("report.txt")).expect("create original");
        fs::File::create(dir.join("report (2).txt")).expect("create second");

        let path = unique_download_path(&dir, "report.txt");

        assert_eq!(path.file_name().and_then(|value| value.to_str()), Some("report (3).txt"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unique_download_path_sanitizes_unsafe_file_name() {
        let dir = std::env::temp_dir().join(format!("colink-download-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create dir");
        let mut file = fs::File::create(dir.join("safe.txt")).expect("create file");
        file.write_all(b"existing").expect("write");

        let path = unique_download_path(&dir, "../safe.txt");

        assert_eq!(path.parent(), Some(dir.as_path()));
        assert!(!path.file_name().and_then(|value| value.to_str()).unwrap_or("").contains('/'));
        assert!(!path.file_name().and_then(|value| value.to_str()).unwrap_or("").contains('\\'));

        let _ = fs::remove_dir_all(dir);
    }
}
