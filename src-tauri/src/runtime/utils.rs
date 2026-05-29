use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use sanitize_filename::sanitize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

const FILE_CHECKSUM_ALGORITHM: &str = "blake3";
const FILE_HASH_BUFFER_SIZE: usize = 1_048_576;

pub(super) fn build_file_checksum(path: &Path) -> AppResult<String> {
    let digest = hash_file_by_algorithm(path, FILE_CHECKSUM_ALGORITHM)?;
    Ok(format!("{FILE_CHECKSUM_ALGORITHM}:{digest}"))
}

pub(super) fn verify_file_checksum(path: &Path, checksum: &str) -> AppResult<bool> {
    let (algorithm, expected) = split_checksum(checksum);
    let actual = hash_file_by_algorithm(path, algorithm)?;
    Ok(actual.eq_ignore_ascii_case(expected))
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

fn split_checksum(checksum: &str) -> (&str, &str) {
    if let Some((algorithm, digest)) = checksum.split_once(':') {
        return (algorithm, digest);
    }

    ("sha256", checksum)
}

fn hash_file_by_algorithm(path: &Path, algorithm: &str) -> AppResult<String> {
    let mut file = fs::File::open(path)?;
    let mut buffer = vec![0_u8; FILE_HASH_BUFFER_SIZE];

    match algorithm {
        "sha256" => {
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
        "blake3" => {
            let mut hasher = blake3::Hasher::new();
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(hasher.finalize().to_hex().to_string())
        }
        _ => Err(AppError::message(format!(
            "unsupported checksum algorithm: {algorithm}"
        ))),
    }
}
