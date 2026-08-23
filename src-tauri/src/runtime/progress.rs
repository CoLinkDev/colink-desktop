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
    next_expected_index > 0
        && ((total_chunks > 0 && next_expected_index >= total_chunks)
            || next_expected_index % FILE_ACK_INTERVAL_CHUNKS == 0)
}

#[cfg(test)]
mod tests {
    use super::{
        acknowledged_file_bytes, calculate_bytes_per_second, should_send_file_ack,
        FILE_ACK_INTERVAL_CHUNKS,
    };
    use crate::models::FILE_CHUNK_SIZE;

    #[test]
    fn calculates_bytes_per_second_from_positive_delta() {
        assert_eq!(calculate_bytes_per_second(1_024, 500), 2_048.0);
        assert_eq!(calculate_bytes_per_second(1_024, 0), 1_024_000.0);
    }

    #[test]
    fn ignores_invalid_progress_samples() {
        assert_eq!(calculate_bytes_per_second(0, 500), 0.0);
        assert_eq!(calculate_bytes_per_second(-1, 500), 0.0);
    }

    #[test]
    fn calculates_acknowledged_file_bytes_without_exceeding_file_size() {
        let file_size = FILE_CHUNK_SIZE as i64 * 2 + 128;
        assert_eq!(
            acknowledged_file_bytes(file_size, 3, 2),
            FILE_CHUNK_SIZE as i64 * 2
        );
        assert_eq!(acknowledged_file_bytes(file_size, 3, 3), file_size);
        assert_eq!(acknowledged_file_bytes(file_size, 3, 10), file_size);
    }

    #[test]
    fn ignores_invalid_acknowledged_byte_inputs() {
        assert_eq!(acknowledged_file_bytes(0, 3, 1), 0);
        assert_eq!(acknowledged_file_bytes(100, 0, 1), 0);
        assert_eq!(acknowledged_file_bytes(100, 3, 0), 0);
    }

    #[test]
    fn sends_file_ack_at_interval_and_finish() {
        assert!(should_send_file_ack(FILE_ACK_INTERVAL_CHUNKS, 20));
        assert!(should_send_file_ack(20, 20));
        assert!(!should_send_file_ack(FILE_ACK_INTERVAL_CHUNKS - 1, 20));
        assert!(!should_send_file_ack(1, 0));
    }
}
