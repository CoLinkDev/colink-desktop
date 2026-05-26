use serde::Serialize;

use crate::models::{FileTransferRecord, FILE_CHUNK_SIZE};

use super::FILE_ACK_INTERVAL_CHUNKS;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TransferProgressPayload {
    pub(super) record: FileTransferRecord,
    pub(super) bytes_per_second: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TransferPreparingPayload {
    pub(super) current: usize,
    pub(super) total: usize,
}

pub(super) fn calculate_bytes_per_second(delta_bytes: i64, duration_ms: i64) -> f64 {
    if delta_bytes <= 0 {
        return 0.0;
    }

    if duration_ms <= 0 {
        return delta_bytes as f64 * 1000.0;
    }

    delta_bytes as f64 * 1000.0 / duration_ms as f64
}

pub(super) fn acknowledged_file_bytes(
    file_size: i64,
    total_chunks: i64,
    next_expected_index: i64,
) -> i64 {
    if file_size <= 0 || total_chunks <= 0 || next_expected_index <= 0 {
        return 0;
    }

    let acknowledged = next_expected_index
        .min(total_chunks)
        .saturating_mul(FILE_CHUNK_SIZE as i64);
    acknowledged.min(file_size)
}

pub(super) fn should_send_file_ack(next_expected_index: i64, total_chunks: i64) -> bool {
    next_expected_index >= total_chunks || next_expected_index % FILE_ACK_INTERVAL_CHUNKS == 0
}
