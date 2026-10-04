use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use tokio::sync::mpsc;

use crate::{
    error::{AppError, AppResult},
    protocol::{CameraCapabilities, CameraEntry, CameraFpsRange, CameraResolution},
    runtime_events::RuntimeEvent,
    sync::MutexExt,
};

#[derive(Clone)]
pub(super) struct CameraCaptureRequest {
    pub session_id: String,
    pub generation: u64,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct CameraCaptureProfile {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Clone)]
pub(super) struct CameraCaptureService {
    state: Arc<Mutex<CaptureState>>,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    capability_cache: Arc<Mutex<HashMap<String, CachedCameraCapabilities>>>,
    enumeration_lock: Arc<Mutex<()>>,
}

#[derive(Clone)]
struct CachedCameraCapabilities {
    capabilities: CameraCapabilities,
    cached_at: Instant,
}

#[derive(Default)]
struct CaptureState {
    active: HashMap<String, ActiveCapture>,
    stopping_cameras: HashMap<String, HashSet<String>>,
    pending_frames: HashMap<String, PendingCameraFrame>,
    frame_events_queued: HashSet<String>,
}

struct ActiveCapture {
    camera_id: String,
    cancelled: Arc<AtomicBool>,
}

struct PendingCameraFrame {
    generation: u64,
    keyframe: bool,
    payload: Vec<u8>,
}

impl CameraCaptureService {
    pub(super) fn new(event_tx: mpsc::UnboundedSender<RuntimeEvent>) -> Self {
        Self {
            state: Arc::new(Mutex::new(CaptureState::default())),
            event_tx,
            capability_cache: Arc::new(Mutex::new(HashMap::new())),
            enumeration_lock: Arc::new(Mutex::new(())),
        }
    }

    pub(super) fn list_devices(&self) -> AppResult<Vec<CameraEntry>> {
        platform::list_devices()
    }

    pub(super) fn list_devices_v2(&self) -> AppResult<Vec<CameraEntry>> {
        const CAPABILITY_CACHE_TTL: Duration = Duration::from_secs(30);

        let _enumeration_guard = self.enumeration_lock.lock_unpoisoned();
        let cached = {
            let now = Instant::now();
            let mut cache = self.capability_cache.lock_unpoisoned();
            cache.retain(|_, entry| {
                now.saturating_duration_since(entry.cached_at) < CAPABILITY_CACHE_TTL
            });
            cache
                .iter()
                .map(|(camera_id, entry)| {
                    (camera_id.clone(), entry.capabilities.clone())
                })
                .collect::<HashMap<_, _>>()
        };
        let cameras = platform::list_devices_v2(&cached)?;
        let visible_camera_ids = cameras
            .iter()
            .map(|camera| camera.camera_id.as_str())
            .collect::<HashSet<_>>();
        let mut cache = self.capability_cache.lock_unpoisoned();
        cache.retain(|camera_id, _| visible_camera_ids.contains(camera_id.as_str()));
        let cached_at = Instant::now();
        for camera in &cameras {
            if cached.contains_key(&camera.camera_id) {
                continue;
            }
            if let Some(capabilities) = camera.capabilities.clone() {
                cache.insert(
                    camera.camera_id.clone(),
                    CachedCameraCapabilities {
                        capabilities,
                        cached_at,
                    },
                );
            }
        }
        Ok(cameras)
    }

    pub(super) fn negotiate(
        &self,
        camera_id: &str,
        width: u32,
        height: u32,
        fps: u32,
    ) -> AppResult<CameraCaptureProfile> {
        platform::negotiate(camera_id, width, height, fps)
    }

    pub(super) fn negotiate_exact(
        &self,
        camera_id: &str,
        width: u32,
        height: u32,
        fps: u32,
    ) -> AppResult<CameraCaptureProfile> {
        platform::negotiate_exact(camera_id, width, height, fps)
    }

    pub(super) fn start(&self, request: CameraCaptureRequest) -> AppResult<()> {
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut state = self.state.lock_unpoisoned();
            state.reserve(&request, cancelled.clone())?;
        }
        tracing::info!(
            session_id = %request.session_id,
            camera_id = %request.camera_id,
            generation = request.generation,
            "native camera capture starting"
        );

        let capture_service = self.clone();
        let event_tx = self.event_tx.clone();
        let session_id = request.session_id.clone();
        let cleanup_session_id = session_id.clone();
        let cleanup_cancelled = cancelled.clone();
        thread::Builder::new()
            .name(format!("camera-{}", &session_id[..session_id.len().min(8)]))
            .spawn(move || {
                let generation = request.generation;
                let frame_service = capture_service.clone();
                let frame_session_id = session_id.clone();
                let frame_cancelled = cancelled.clone();
                let result = platform::capture(request, cancelled.clone(), move |keyframe, payload| {
                    if !frame_cancelled.load(Ordering::Acquire) {
                        frame_service.queue_frame(
                            &frame_session_id,
                            generation,
                            keyframe,
                            payload,
                            &frame_cancelled,
                        );
                    }
                });

                if let Err(error) = result {
                    if !cancelled.load(Ordering::Acquire) {
                        tracing::warn!(
                            session_id = %session_id,
                            generation,
                            error = %error,
                            "native camera capture stopped"
                        );
                        let _ = event_tx.send(RuntimeEvent::NativeCameraFailed {
                            session_id: session_id.clone(),
                            generation,
                            message: error.to_string(),
                        });
                    }
                }

                if capture_service.finish_capture(&session_id, &cancelled) {
                    tracing::info!(%session_id, generation, "native camera capture stopped");
                    let _ = event_tx.send(RuntimeEvent::NativeCameraStopped {
                        session_id,
                        generation,
                    });
                }
            })
            .map_err(|error| {
                self.finish_capture(&cleanup_session_id, &cleanup_cancelled);
                self.release_stopped_camera(&cleanup_session_id);
                AppError::message(error.to_string())
            })?;
        Ok(())
    }

    pub(super) fn stop(&self, session_id: &str) {
        let cancelled = {
            let mut state = self.state.lock_unpoisoned();
            state.request_stop(session_id)
        };
        if let Some(cancelled) = cancelled {
            tracing::info!(%session_id, "native camera capture stop requested");
            cancelled.store(true, Ordering::Release);
        }
    }

    pub(super) fn take_frame(&self, session_id: &str) -> Option<(u64, bool, Vec<u8>)> {
        let mut state = self.state.lock_unpoisoned();
        state.take_frame(session_id)
    }

    pub(super) fn release_stopped_camera(&self, session_id: &str) {
        let mut state = self.state.lock_unpoisoned();
        state.release_stopped_camera(session_id);
    }

    fn queue_frame(
        &self,
        session_id: &str,
        generation: u64,
        keyframe: bool,
        payload: Vec<u8>,
        cancelled: &Arc<AtomicBool>,
    ) {
        let should_notify = {
            let mut state = self.state.lock_unpoisoned();
            state.queue_frame(session_id, generation, keyframe, payload, cancelled)
        };
        if should_notify {
            let _ = self.event_tx.send(RuntimeEvent::NativeCameraFramesReady {
                session_id: session_id.to_string(),
            });
        }
    }

    fn finish_capture(
        &self,
        session_id: &str,
        cancelled: &Arc<AtomicBool>,
    ) -> bool {
        let mut state = self.state.lock_unpoisoned();
        state.finish_capture(session_id, cancelled)
    }
}

const MAX_CAMERA_RESOLUTION_TIERS: usize = 5;

fn camera_probe_candidates(
    modes: impl IntoIterator<Item = CameraCaptureProfile>,
) -> Vec<CameraCaptureProfile> {
    let Some(capabilities) = promised_capabilities(modes) else {
        return Vec::new();
    };
    let target_area = f64::from(1280 * 720);
    let mut resolutions = capabilities.resolutions;
    resolutions.sort_by(|left, right| {
        let area = |resolution: &CameraResolution| {
            f64::from(resolution.width) * f64::from(resolution.height)
        };
        (area(left).ln() - target_area.ln())
            .abs()
            .total_cmp(&(area(right).ln() - target_area.ln()).abs())
            .then_with(|| {
                (u64::from(right.width) * u64::from(right.height))
                    .cmp(&(u64::from(left.width) * u64::from(left.height)))
            })
    });

    let resolutions = resolutions
        .into_iter()
        .filter_map(|resolution| {
            let mut rates = resolution
                .fps
                .into_iter()
                .flat_map(|range| [range.min, range.max])
                .filter(|fps| *fps > 0)
                .collect::<Vec<_>>();
            rates.sort_unstable();
            rates.dedup();
            let minimum = *rates.first()?;
            let normal = rates
                .iter()
                .copied()
                .filter(|fps| *fps <= 30)
                .max()
                .unwrap_or(minimum);
            let low = rates
                .iter()
                .copied()
                .filter(|fps| *fps <= 15)
                .max()
                .unwrap_or(minimum);
            let high = *rates.last()?;
            Some((resolution.width, resolution.height, [normal, low, high]))
        })
        .collect::<Vec<_>>();

    let mut candidates = Vec::with_capacity(resolutions.len() * 3);
    let mut selected = HashSet::new();
    for stage in 0..3 {
        for (width, height, rates) in &resolutions {
            let profile = CameraCaptureProfile {
                width: *width,
                height: *height,
                fps: rates[stage],
            };
            if selected.insert(profile) {
                candidates.push(profile);
            }
        }
    }
    candidates
}

fn promised_capabilities(
    modes: impl IntoIterator<Item = CameraCaptureProfile>,
) -> Option<CameraCapabilities> {
    let mut grouped = HashMap::<(u32, u32), HashSet<u32>>::new();
    for mode in modes {
        if mode.width == 0 || mode.height == 0 || mode.fps == 0 {
            continue;
        }
        grouped
            .entry((mode.width, mode.height))
            .or_default()
            .insert(mode.fps);
    }
    let mut resolutions = grouped
        .into_iter()
        .map(|((width, height), rates)| {
            let mut rates = rates.into_iter().collect::<Vec<_>>();
            rates.sort_unstable();
            CameraResolution {
                width,
                height,
                fps: rates
                    .into_iter()
                    .map(|fps| CameraFpsRange { min: fps, max: fps })
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    resolutions.sort_by_key(|resolution| {
        (u64::from(resolution.width) * u64::from(resolution.height), resolution.width, resolution.height)
    });
    if resolutions.is_empty() {
        return None;
    }
    if resolutions.len() > MAX_CAMERA_RESOLUTION_TIERS {
        let tier_count = MAX_CAMERA_RESOLUTION_TIERS.min(resolutions.len());
        let mut selected = vec![0, resolutions.len() - 1];
        let log_area = |index: usize| {
            let resolution = &resolutions[index];
            (f64::from(resolution.width) * f64::from(resolution.height)).ln()
        };
        let minimum = log_area(0);
        let maximum = log_area(resolutions.len() - 1);
        for tier in 1..tier_count - 1 {
            let target = minimum
                + (maximum - minimum) * tier as f64 / (tier_count - 1) as f64;
            let nearest = (0..resolutions.len()).min_by(|left, right| {
                (log_area(*left) - target)
                    .abs()
                    .total_cmp(&(log_area(*right) - target).abs())
                    .then_with(|| left.cmp(right))
            });
            if let Some(nearest) = nearest.filter(|index| !selected.contains(index)) {
                selected.push(nearest);
            }
        }
        while selected.len() < tier_count {
            let next = (0..resolutions.len())
                .filter(|index| !selected.contains(index))
                .max_by(|left, right| {
                    let separation = |index: usize| {
                        selected
                            .iter()
                            .map(|selected_index| (log_area(index) - log_area(*selected_index)).abs())
                            .fold(f64::INFINITY, f64::min)
                    };
                    separation(*left)
                        .total_cmp(&separation(*right))
                        .then_with(|| right.cmp(left))
                });
            let Some(next) = next else { break; };
            selected.push(next);
        }
        selected.sort_unstable();
        resolutions = selected
            .into_iter()
            .map(|index| resolutions[index].clone())
            .collect();
    }
    resolutions.reverse();
    Some(CameraCapabilities {
        resolutions,
        fps_range: None,
    })
}

impl CaptureState {
    fn reserve(&mut self, request: &CameraCaptureRequest, cancelled: Arc<AtomicBool>) -> AppResult<()> {
        if self.active.contains_key(&request.session_id) {
            return Err(AppError::message("camera capture is already active"));
        }
        if let Some(sessions) = self.stopping_cameras.get(&request.camera_id) {
            let restarting_own_capture = sessions.len() == 1 && sessions.contains(&request.session_id);
            if !restarting_own_capture {
                return Err(AppError::message("camera is still shutting down"));
            }
            let should_remove = self
                .stopping_cameras
                .get_mut(&request.camera_id)
                .is_some_and(|sessions| {
                    sessions.remove(&request.session_id);
                    sessions.is_empty()
                });
            if should_remove {
                self.stopping_cameras.remove(&request.camera_id);
            }
        }
        self.active.insert(
            request.session_id.clone(),
            ActiveCapture {
                camera_id: request.camera_id.clone(),
                cancelled,
            },
        );
        Ok(())
    }

    fn request_stop(&mut self, session_id: &str) -> Option<Arc<AtomicBool>> {
        self.pending_frames.remove(session_id);
        self.frame_events_queued.remove(session_id);
        let active = self.active.get(session_id)?;
        let camera_id = active.camera_id.clone();
        let cancelled = active.cancelled.clone();
        self.stopping_cameras
            .entry(camera_id)
            .or_default()
            .insert(session_id.to_string());
        Some(cancelled)
    }

    fn take_frame(&mut self, session_id: &str) -> Option<(u64, bool, Vec<u8>)> {
        self.frame_events_queued.remove(session_id);
        self.pending_frames
            .remove(session_id)
            .map(|frame| (frame.generation, frame.keyframe, frame.payload))
    }

    fn queue_frame(
        &mut self,
        session_id: &str,
        generation: u64,
        keyframe: bool,
        payload: Vec<u8>,
        cancelled: &Arc<AtomicBool>,
    ) -> bool {
        let Some(active) = self.active.get(session_id) else { return false; };
        if !Arc::ptr_eq(&active.cancelled, cancelled) || cancelled.load(Ordering::Acquire) {
            return false;
        }
        let replace = self
            .pending_frames
            .get(session_id)
            .is_none_or(|frame| keyframe || !frame.keyframe);
        if replace {
            self.pending_frames.insert(
                session_id.to_string(),
                PendingCameraFrame {
                    generation,
                    keyframe,
                    payload,
                },
            );
        }
        self.frame_events_queued.insert(session_id.to_string())
    }

    fn finish_capture(&mut self, session_id: &str, cancelled: &Arc<AtomicBool>) -> bool {
        if !self
            .active
            .get(session_id)
            .is_some_and(|active| Arc::ptr_eq(&active.cancelled, cancelled))
        {
            return false;
        }
        self.active.remove(session_id);
        self.pending_frames.remove(session_id);
        self.frame_events_queued.remove(session_id);
        // Keep a stop request reserved until the runtime handles NativeCameraStopped.
        true
    }

    fn release_stopped_camera(&mut self, session_id: &str) {
        if self.active.contains_key(session_id) {
            return;
        }
        self.stopping_cameras.retain(|_, sessions| {
            sessions.remove(session_id);
            !sessions.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::{atomic::AtomicBool, Arc},
    };

    use super::{
        camera_probe_candidates, promised_capabilities, CameraCaptureProfile,
        CameraCaptureRequest, CaptureState, MAX_CAMERA_RESOLUTION_TIERS,
    };

    fn profile(width: u32, height: u32, fps: u32) -> CameraCaptureProfile {
        CameraCaptureProfile { width, height, fps }
    }

    fn request(session_id: &str, camera_id: &str) -> CameraCaptureRequest {
        CameraCaptureRequest {
            session_id: session_id.to_string(),
            generation: 1,
            camera_id: camera_id.to_string(),
            width: 640,
            height: 360,
            fps: 8,
        }
    }

    fn reserve(state: &mut CaptureState, session_id: &str, camera_id: &str) -> Arc<AtomicBool> {
        let cancelled = Arc::new(AtomicBool::new(false));
        state
            .reserve(&request(session_id, camera_id), cancelled.clone())
            .expect("reserve capture");
        cancelled
    }

    #[test]
    fn stopping_capture_blocks_new_sessions_until_the_native_thread_exits() {
        let mut state = CaptureState::default();
        let first = reserve(&mut state, "first", "camera");
        reserve(&mut state, "second", "camera");

        let stopped = state.request_stop("first").expect("request first stop");
        assert!(Arc::ptr_eq(&stopped, &first));
        assert!(state.reserve(&request("third", "camera"), Arc::new(AtomicBool::new(false))).is_err());

        assert!(state.finish_capture("first", &first));
        assert!(state.reserve(&request("third", "camera"), Arc::new(AtomicBool::new(false))).is_err());

        state.release_stopped_camera("first");
        reserve(&mut state, "third", "camera");
        assert!(state.active.contains_key("second"));
        assert!(state.active.contains_key("third"));
    }

    #[test]
    fn reconfiguration_restarts_only_after_its_previous_capture_has_finished() {
        let mut state = CaptureState::default();
        let first = reserve(&mut state, "session", "camera");

        state.request_stop("session").expect("request stop");
        assert!(state.reserve(&request("session", "camera"), Arc::new(AtomicBool::new(false))).is_err());

        assert!(state.finish_capture("session", &first));
        reserve(&mut state, "session", "camera");
        assert!(!state.stopping_cameras.contains_key("camera"));
    }

    #[test]
    fn frame_queue_is_bounded_and_preserves_a_keyframe() {
        let mut state = CaptureState::default();
        let cancelled = reserve(&mut state, "session", "camera");

        assert!(state.queue_frame("session", 1, false, vec![1], &cancelled));
        assert!(!state.queue_frame("session", 1, false, vec![2], &cancelled));
        assert!(!state.queue_frame("session", 1, true, vec![3], &cancelled));
        assert!(!state.queue_frame("session", 1, false, vec![4], &cancelled));

        assert_eq!(state.take_frame("session"), Some((1, true, vec![3])));
        assert!(state.queue_frame("session", 1, false, vec![5], &cancelled));
    }

    #[test]
    fn active_session_cannot_be_replaced() {
        let mut state = CaptureState::default();
        reserve(&mut state, "session", "camera");

        assert!(state.reserve(&request("session", "camera"), Arc::new(AtomicBool::new(false))).is_err());
    }

    #[test]
    fn promised_capabilities_keep_valid_modes_and_singleton_frame_rates() {
        let capabilities = promised_capabilities([
            profile(1280, 720, 30),
            profile(640, 360, 15),
            profile(1280, 720, 15),
            profile(1280, 720, 30),
            profile(0, 720, 30),
        ])
        .expect("camera capabilities");

        assert_eq!(capabilities.resolutions.len(), 2);
        assert_eq!((capabilities.resolutions[0].width, capabilities.resolutions[0].height), (1280, 720));
        assert_eq!(
            capabilities.resolutions[0]
                .fps
                .iter()
                .map(|range| (range.min, range.max))
                .collect::<Vec<_>>(),
            vec![(15, 15), (30, 30)],
        );
        assert_eq!((capabilities.resolutions[1].width, capabilities.resolutions[1].height), (640, 360));
    }

    #[test]
    fn promised_capabilities_limit_resolutions_and_spread_them_logarithmically() {
        let capabilities = promised_capabilities([
            profile(160, 90, 30),
            profile(240, 135, 30),
            profile(320, 180, 30),
            profile(480, 270, 30),
            profile(640, 360, 30),
            profile(960, 540, 30),
            profile(1280, 720, 30),
            profile(1920, 1080, 30),
        ])
        .expect("camera capabilities");

        assert_eq!(capabilities.resolutions.len(), 5);
        assert_eq!(
            (capabilities.resolutions[0].width, capabilities.resolutions[0].height),
            (1920, 1080),
        );
        assert_eq!(
            (
                capabilities.resolutions[4].width,
                capabilities.resolutions[4].height,
            ),
            (160, 90),
        );
        assert!(capabilities.resolutions.windows(2).all(|pair| {
            let larger_area = f64::from(pair[0].width) * f64::from(pair[0].height);
            let smaller_area = f64::from(pair[1].width) * f64::from(pair[1].height);
            larger_area / smaller_area <= 4.01
        }));
    }

    #[test]
    fn promised_capabilities_return_none_without_a_valid_mode() {
        assert!(promised_capabilities([profile(0, 0, 0)]).is_none());
    }

    #[test]
    fn camera_probe_candidates_prioritize_normal_then_low_and_high_frame_rates() {
        let candidates = camera_probe_candidates(
            [(640, 360), (1280, 720), (1920, 1080)]
                .into_iter()
                .flat_map(|(width, height)| {
                    [10, 15, 24, 30, 60]
                        .into_iter()
                        .map(move |fps| profile(width, height, fps))
                }),
        );

        assert_eq!(
            candidates,
            vec![
                profile(1280, 720, 30),
                profile(1920, 1080, 30),
                profile(640, 360, 30),
                profile(1280, 720, 15),
                profile(1920, 1080, 15),
                profile(640, 360, 15),
                profile(1280, 720, 60),
                profile(1920, 1080, 60),
                profile(640, 360, 60),
            ],
        );
    }

    #[test]
    fn camera_probe_candidates_limit_resolution_count_and_remove_duplicates() {
        let candidates = camera_probe_candidates(
            [
                (160, 90),
                (240, 135),
                (320, 180),
                (480, 270),
                (640, 360),
                (960, 540),
                (1280, 720),
                (1920, 1080),
            ]
            .into_iter()
            .flat_map(|(width, height)| {
                [30, 30]
                    .into_iter()
                    .map(move |fps| profile(width, height, fps))
            }),
        );

        assert_eq!(candidates.len(), MAX_CAMERA_RESOLUTION_TIERS);
        assert_eq!(candidates.iter().copied().collect::<HashSet<_>>().len(), candidates.len());
    }
}

#[cfg(windows)]
mod platform {
    use std::{
        collections::{HashMap, HashSet},
        mem::ManuallyDrop,
        ptr,
        sync::{atomic::{AtomicBool, Ordering}, Arc},
        thread,
        time::{Duration, Instant},
    };

    use windows::{
        core::{Error as WindowsError, Interface},
        Win32::{
            Media::MediaFoundation::{
                MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
                MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFShutdown, MFStartup,
                MFTEnumEx, IMFActivate, IMFMediaBuffer, IMFMediaEventGenerator, IMFMediaSource,
                IMFMediaType, IMFSample, IMFSourceReader, IMFTransform, MFT_ENUM_FLAG, MFT_ENUM_FLAG_ASYNCMFT,
                MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
                MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_END_STREAMING,
                MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
                MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
                MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_E_TRANSFORM_NEED_MORE_INPUT,
                MF_E_NO_EVENTS_AVAILABLE, MF_EVENT_FLAG_NO_WAIT,
                MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
                MF_MT_MAX_KEYFRAME_SPACING,
                MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_SUBTYPE,
                MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS,
                MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK,
                MFT_SET_TYPE_TEST_ONLY,
                MFSampleExtension_CleanPoint, MF_SOURCE_READER_ALL_STREAMS,
                MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM,
                METransformDrainComplete, METransformHaveOutput, METransformNeedInput, MFSTARTUP_FULL, MF_VERSION,
                MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_NV12,
            },
            System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED},
        },
    };

    use crate::{
        error::{AppError, AppResult},
        protocol::{CameraCapabilities, CameraEntry},
    };

    use super::{
        camera_probe_candidates, promised_capabilities, CameraCaptureProfile,
        CameraCaptureRequest,
    };

    const HNS_PER_SECOND: u64 = 10_000_000;
    const CAMERA_FORMAT_OPEN_ATTEMPTS: usize = 2;
    const CAMERA_FORMAT_OPEN_RETRY_DELAY: Duration = Duration::from_millis(250);
    const CAMERA_ENUMERATION_WORKERS: usize = 2;
    const CAMERA_PROBE_BUDGET: Duration = Duration::from_secs(2);

    pub(super) fn list_devices() -> AppResult<Vec<CameraEntry>> {
        let _com = ComApartment::new()?;
        let _media_foundation = MediaFoundation::new()?;
        enumerate_devices().map(|devices| {
            devices
                .into_iter()
                .map(|device| CameraEntry {
                    camera_id: device.id,
                    label: device.label,
                    position: None,
                    capabilities: None,
                })
                .collect()
        })
    }

    pub(super) fn list_devices_v2(
        cached: &HashMap<String, CameraCapabilities>,
    ) -> AppResult<Vec<CameraEntry>> {
        let devices = {
            let _com = ComApartment::new()?;
            let _media_foundation = MediaFoundation::new()?;
            enumerate_devices()?
        };
        let mut cameras = vec![None; devices.len()];
        let mut pending = Vec::new();
        for (index, device) in devices.into_iter().enumerate() {
            if let Some(capabilities) = cached.get(&device.id) {
                cameras[index] = Some(CameraEntry {
                    camera_id: device.id,
                    label: device.label,
                    position: None,
                    capabilities: Some(capabilities.clone()),
                });
            } else {
                pending.push((index, device));
            }
        }

        let worker_count = CAMERA_ENUMERATION_WORKERS.min(pending.len());
        let mut buckets = (0..worker_count).map(|_| Vec::new()).collect::<Vec<_>>();
        for (offset, pending_device) in pending.into_iter().enumerate() {
            buckets[offset % worker_count].push(pending_device);
        }
        let mut workers = Vec::with_capacity(worker_count);
        for (worker_index, bucket) in buckets.into_iter().enumerate() {
            workers.push(
                thread::Builder::new()
                    .name(format!("camera-probe-{worker_index}"))
                    .spawn(move || probe_camera_batch(bucket))
                    .map_err(|error| AppError::message(error.to_string()))?,
            );
        }

        let mut first_worker_error = None;
        for worker in workers {
            match worker.join() {
                Ok(Ok(entries)) => {
                    for (index, camera) in entries {
                        cameras[index] = Some(camera);
                    }
                }
                Ok(Err(error)) => {
                    tracing::warn!(%error, "camera capability worker failed");
                    first_worker_error.get_or_insert(error);
                }
                Err(_) => {
                    tracing::warn!("camera capability worker panicked");
                    first_worker_error.get_or_insert_with(|| {
                        AppError::message("camera capability worker panicked")
                    });
                }
            }
        }
        if cameras.iter().all(Option::is_none) {
            if let Some(error) = first_worker_error {
                return Err(error);
            }
        }
        Ok(cameras.into_iter().flatten().collect())
    }

    fn probe_camera_batch(
        devices: Vec<(usize, NativeCameraDevice)>,
    ) -> AppResult<Vec<(usize, CameraEntry)>> {
        let _com = ComApartment::new()?;
        let _media_foundation = MediaFoundation::new()?;
        let mut cameras = Vec::new();
        for (index, device) in devices {
            let started_at = Instant::now();
            let deadline = started_at + CAMERA_PROBE_BUDGET;
            match enumerate_encodable_camera_modes(&device.id, deadline) {
                Ok(modes) => {
                    if let Some(capabilities) = promised_capabilities(modes) {
                        cameras.push((
                            index,
                            CameraEntry {
                                camera_id: device.id.clone(),
                                label: device.label.clone(),
                                position: None,
                                capabilities: Some(capabilities),
                            },
                        ));
                    } else {
                        tracing::warn!(
                            camera_id = %device.id,
                            camera_label = %device.label,
                            "camera omitted because no promised mode could be verified"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        camera_id = %device.id,
                        camera_label = %device.label,
                        %error,
                        "camera omitted because its promised modes could not be verified"
                    );
                }
            }
            let elapsed = started_at.elapsed();
            if elapsed > CAMERA_PROBE_BUDGET {
                tracing::warn!(
                    camera_id = %device.id,
                    camera_label = %device.label,
                    elapsed_ms = elapsed.as_millis(),
                    budget_ms = CAMERA_PROBE_BUDGET.as_millis(),
                    "camera capability probe exceeded its soft time budget"
                );
            }
        }
        Ok(cameras)
    }

    pub(super) fn negotiate(
        camera_id: &str,
        width: u32,
        height: u32,
        fps: u32,
    ) -> AppResult<CameraCaptureProfile> {
        let _com = ComApartment::new()?;
        let _media_foundation = MediaFoundation::new()?;
        let profile = select_camera_profile(camera_id, width, height, fps)?;
        activate_h264_encoder()?;
        Ok(profile)
    }

    pub(super) fn negotiate_exact(
        camera_id: &str,
        width: u32,
        height: u32,
        fps: u32,
    ) -> AppResult<CameraCaptureProfile> {
        let _com = ComApartment::new()?;
        let _media_foundation = MediaFoundation::new()?;
        verify_encodable_camera_mode(
            camera_id,
            CameraCaptureProfile { width, height, fps },
        )
    }

    pub(super) fn capture(
        request: CameraCaptureRequest,
        cancelled: Arc<AtomicBool>,
        mut emit_frame: impl FnMut(bool, Vec<u8>),
    ) -> AppResult<()> {
        let _com = ComApartment::new()?;
        let _media_foundation = MediaFoundation::new()?;
        let activate = find_device(&request.camera_id)?;
        let source = MediaSourceSession(
            unsafe { activate.ActivateObject::<IMFMediaSource>() }.map_err(windows_error)?,
        );
        let reader = unsafe { MFCreateSourceReaderFromMediaSource(&source.0, None) }
            .map_err(windows_error)?;
        unsafe {
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(windows_error)?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
                .map_err(windows_error)?;
        }

        let input_type = native_nv12_type(&reader, CameraCaptureProfile {
            width: request.width,
            height: request.height,
            fps: request.fps,
        })?;
        unsafe {
            reader
                .SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    None,
                    &input_type,
                )
                .map_err(|error| camera_error("configure camera source type", error))?;
        }
        let actual_input_type = unsafe {
            reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
        }
        .map_err(windows_error)?;

        let encoder = activate_h264_encoder()?;
        let output_type = h264_video_type(request.width, request.height, request.fps)?;
        unsafe {
            encoder
                .transform
                .SetOutputType(0, &output_type, 0)
                .map_err(|error| camera_error("set H.264 output type", error))?;
            encoder
                .transform
                .SetInputType(0, &actual_input_type, 0)
                .map_err(|error| camera_error("set H.264 input type", error))?;
            encoder
                .transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)
                .map_err(|error| camera_error("flush H.264 encoder", error))?;
            encoder.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|error| camera_error("begin H.264 stream", error))?;
            encoder.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|error| camera_error("start H.264 stream", error))?;
        }

        let mut force_keyframe_at = 0_u64;
        if let Some(event_generator) = encoder.event_generator.as_ref() {
            process_async_encoder(
                &encoder.transform,
                event_generator,
                &reader,
                &cancelled,
                &mut force_keyframe_at,
                &mut emit_frame,
            )?;
        } else {
            while !cancelled.load(Ordering::Acquire) {
                let Some(sample) = read_camera_sample(&reader, &mut force_keyframe_at)? else { continue; };
                unsafe { encoder.transform.ProcessInput(0, &sample, 0) }
                    .map_err(|error| camera_error("submit camera frame to H.264 encoder", error))?;

                while let Some((keyframe, payload)) = take_encoded_sample(&encoder.transform)? {
                    if !cancelled.load(Ordering::Acquire) {
                        emit_frame(keyframe, payload);
                    }
                }
            }
        }

        shutdown_h264_encoder(&encoder)?;
        unsafe { reader.Flush(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32) }
            .map_err(|error| camera_error("flush camera source", error))?;
        Ok(())
    }

    fn read_camera_sample(
        reader: &IMFSourceReader,
        force_keyframe_at: &mut u64,
    ) -> AppResult<Option<IMFSample>> {
        loop {
            let mut flags = 0_u32;
            let mut sample = None;
            unsafe {
                reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut flags),
                        None,
                        Some(&mut sample),
                    )
                    .map_err(|error| camera_error("read camera frame", error))?;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Ok(None);
            }
            let Some(sample) = sample else { continue; };
            let timestamp = unsafe { sample.GetSampleTime() }.unwrap_or_default().max(0) as u64;
            if timestamp >= *force_keyframe_at {
                unsafe {
                    let _ = sample.SetUINT32(&MFSampleExtension_CleanPoint, 1);
                }
                *force_keyframe_at = timestamp.saturating_add(HNS_PER_SECOND);
            }
            return Ok(Some(sample));
        }
    }

    fn process_async_encoder<F>(
        encoder: &IMFTransform,
        event_generator: &IMFMediaEventGenerator,
        reader: &IMFSourceReader,
        cancelled: &AtomicBool,
        force_keyframe_at: &mut u64,
        emit_frame: &mut F,
    ) -> AppResult<()>
    where
        F: FnMut(bool, Vec<u8>),
    {
        while !cancelled.load(Ordering::Acquire) {
            let event = match unsafe { event_generator.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => event,
                Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(error) => return Err(camera_error("wait for H.264 encoder event", error)),
            };
            unsafe { event.GetStatus() }
                .map_err(|error| camera_error("read H.264 encoder event status", error))?
                .ok()
                .map_err(|error| camera_error("H.264 encoder reported a failure", error))?;
            match unsafe { event.GetType() }
                .map_err(|error| camera_error("read H.264 encoder event type", error))?
            {
                event_type if event_type == METransformNeedInput.0 as u32 => {
                    let Some(sample) = read_camera_sample(reader, force_keyframe_at)? else { continue; };
                    unsafe { encoder.ProcessInput(0, &sample, 0) }
                        .map_err(|error| camera_error("submit camera frame to H.264 encoder", error))?;
                }
                event_type if event_type == METransformHaveOutput.0 as u32 => {
                    // An asynchronous MFT emits one METransformHaveOutput event per output
                    // sample. Calling ProcessOutput again before another event is invalid.
                    if let Some((keyframe, payload)) = take_encoded_sample(encoder)? {
                        if !cancelled.load(Ordering::Acquire) {
                            emit_frame(keyframe, payload);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn shutdown_h264_encoder(encoder: &H264Encoder) -> AppResult<()> {
        unsafe {
            encoder
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .map_err(|error| camera_error("notify H.264 encoder end of stream", error))?;
        }

        if let Some(event_generator) = encoder.event_generator.as_ref() {
            drain_async_encoder(&encoder.transform, event_generator)?;
        } else {
            drain_sync_encoder(&encoder.transform)?;
        }

        unsafe {
            encoder
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0)
                .map_err(|error| camera_error("end H.264 stream", error))?;
        }
        Ok(())
    }

    fn drain_async_encoder(
        encoder: &IMFTransform,
        event_generator: &IMFMediaEventGenerator,
    ) -> AppResult<()> {
        unsafe { encoder.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0) }
            .map_err(|error| camera_error("drain H.264 encoder", error))?;

        loop {
            let event = unsafe { event_generator.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)) }
                .map_err(|error| camera_error("wait for H.264 encoder drain", error))?;
            unsafe { event.GetStatus() }
                .map_err(|error| camera_error("read H.264 encoder drain status", error))?
                .ok()
                .map_err(|error| camera_error("H.264 encoder failed while draining", error))?;
            match unsafe { event.GetType() }
                .map_err(|error| camera_error("read H.264 encoder drain event", error))?
            {
                event_type if event_type == METransformHaveOutput.0 as u32 => {
                    let _ = take_encoded_sample(encoder)?;
                }
                event_type if event_type == METransformDrainComplete.0 as u32 => return Ok(()),
                _ => {}
            }
        }
    }

    fn drain_sync_encoder(encoder: &IMFTransform) -> AppResult<()> {
        unsafe { encoder.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0) }
            .map_err(|error| camera_error("drain H.264 encoder", error))?;
        while take_encoded_sample(encoder)?.is_some() {}
        Ok(())
    }

    struct H264Encoder {
        transform: IMFTransform,
        event_generator: Option<IMFMediaEventGenerator>,
    }

    #[derive(Clone)]
    struct NativeCameraDevice {
        id: String,
        label: String,
    }

    struct ComApartment;

    impl ComApartment {
        fn new() -> AppResult<Self> {
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .ok()
                .map_err(windows_error)?;
            Ok(Self)
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    struct MediaFoundation;

    impl MediaFoundation {
        fn new() -> AppResult<Self> {
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
                .map_err(windows_error)?;
            Ok(Self)
        }
    }

    impl Drop for MediaFoundation {
        fn drop(&mut self) {
            unsafe {
                let _ = MFShutdown();
            }
        }
    }

    struct MediaSourceSession(IMFMediaSource);

    impl Drop for MediaSourceSession {
        fn drop(&mut self) {
            unsafe {
                let _ = self.0.Shutdown();
            }
        }
    }

    fn enumerate_devices() -> AppResult<Vec<NativeCameraDevice>> {
        let attributes = create_attributes(1)?;
        unsafe {
            attributes
                .SetGUID(
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                )
                .map_err(windows_error)?;
        }
        let activates = enum_device_activations(&attributes)?;
        activates
            .into_iter()
            .map(|activate| {
                Ok(NativeCameraDevice {
                    id: get_attribute_string(
                        &activate,
                        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                    )?,
                    label: get_attribute_string(&activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)?,
                })
            })
            .collect()
    }

    fn find_device(camera_id: &str) -> AppResult<IMFActivate> {
        let attributes = create_attributes(1)?;
        unsafe {
            attributes
                .SetGUID(
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                )
                .map_err(windows_error)?;
        }
        enum_device_activations(&attributes)?
            .into_iter()
            .find(|activate| {
                get_attribute_string(
                    activate,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                )
                .is_ok_and(|id| id == camera_id)
            })
            .ok_or_else(|| AppError::message("selected camera is no longer available"))
    }

    fn select_camera_profile(
        camera_id: &str,
        requested_width: u32,
        requested_height: u32,
        requested_fps: u32,
    ) -> AppResult<CameraCaptureProfile> {
        let modes = enumerate_camera_modes(camera_id)?;
        let requested = CameraCaptureProfile {
            width: requested_width,
            height: requested_height,
            fps: requested_fps,
        };
        modes
            .into_iter()
            .min_by_key(|mode| camera_mode_score(mode, requested))
            .ok_or_else(|| AppError::message("camera has no native NV12 capture mode"))
    }

    fn enumerate_camera_modes(camera_id: &str) -> AppResult<Vec<CameraCaptureProfile>> {
        let (_source, reader) = camera_format_reader(camera_id)?;
        let mut modes = HashSet::new();
        for index in 0.. {
            let media_type = match unsafe {
                reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, index)
            } {
                Ok(media_type) => media_type,
                Err(_) => break,
            };
            if let Some(profile) = native_nv12_profile(&media_type) {
                modes.insert(profile);
            }
        }
        Ok(modes.into_iter().collect())
    }

    fn enumerate_encodable_camera_modes(
        camera_id: &str,
        deadline: Instant,
    ) -> AppResult<Vec<CameraCaptureProfile>> {
        let (_source, reader) = camera_format_reader_until(camera_id, Some(deadline))?;
        if Instant::now() >= deadline {
            return Err(AppError::message("camera capability probe timed out"));
        }
        let mut native_types = HashMap::new();
        for index in 0.. {
            if Instant::now() >= deadline {
                break;
            }
            let media_type = match unsafe {
                reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, index)
            } {
                Ok(media_type) => media_type,
                Err(_) => break,
            };
            let Some(profile) = native_nv12_profile(&media_type) else { continue; };
            native_types.entry(profile).or_insert(media_type);
        }
        let candidates = camera_probe_candidates(native_types.keys().copied());
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        if Instant::now() >= deadline {
            return Err(AppError::message("camera capability probe timed out"));
        }
        let encoder = activate_h264_encoder()?;
        let mut modes = Vec::new();
        for profile in candidates {
            if Instant::now() >= deadline {
                break;
            }
            let Some(media_type) = native_types.get(&profile) else { continue; };
            if unsafe {
                reader.SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    None,
                    media_type,
                )
            }
            .is_err()
            {
                continue;
            }
            let Ok(actual_input_type) = (unsafe {
                reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
            }) else {
                continue;
            };
            if native_nv12_profile(&actual_input_type) != Some(profile) {
                continue;
            }
            if h264_encoder_accepts(&encoder.transform, &actual_input_type, profile)? {
                modes.push(profile);
            }
        }
        if modes.is_empty() && Instant::now() >= deadline {
            return Err(AppError::message("camera capability probe timed out"));
        }
        Ok(modes)
    }

    fn verify_encodable_camera_mode(
        camera_id: &str,
        profile: CameraCaptureProfile,
    ) -> AppResult<CameraCaptureProfile> {
        let (_source, reader) = camera_format_reader(camera_id)?;
        let media_type = native_nv12_type(&reader, profile)
            .map_err(|_| AppError::message("requested camera mode was not promised"))?;
        unsafe {
            reader.SetCurrentMediaType(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                None,
                &media_type,
            )
        }
        .map_err(|_| AppError::message("requested camera mode was not promised"))?;
        let actual_input_type = unsafe {
            reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
        }
        .map_err(windows_error)?;
        if native_nv12_profile(&actual_input_type) != Some(profile) {
            return Err(AppError::message("requested camera mode was not promised"));
        }
        let encoder = activate_h264_encoder()?;
        if !h264_encoder_accepts(&encoder.transform, &actual_input_type, profile)? {
            return Err(AppError::message("requested camera mode was not promised"));
        }
        Ok(profile)
    }

    fn camera_format_reader(camera_id: &str) -> AppResult<(MediaSourceSession, IMFSourceReader)> {
        camera_format_reader_until(camera_id, None)
    }

    fn camera_format_reader_until(
        camera_id: &str,
        deadline: Option<Instant>,
    ) -> AppResult<(MediaSourceSession, IMFSourceReader)> {
        let mut last_error = None;
        for attempt in 1..=CAMERA_FORMAT_OPEN_ATTEMPTS {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(last_error
                    .unwrap_or_else(|| AppError::message("camera capability probe timed out")));
            }
            let result = (|| {
                let activate = find_device(camera_id)?;
                let source = MediaSourceSession(
                    unsafe { activate.ActivateObject::<IMFMediaSource>() }
                        .map_err(|error| camera_error("open camera for format negotiation", error))?,
                );
                let reader = unsafe { MFCreateSourceReaderFromMediaSource(&source.0, None) }
                    .map_err(|error| camera_error("create camera format reader", error))?;
                Ok((source, reader))
            })();
            match result {
                Ok(reader) => return Ok(reader),
                Err(error) if attempt < CAMERA_FORMAT_OPEN_ATTEMPTS => {
                    if deadline.is_some_and(|deadline| {
                        Instant::now() + CAMERA_FORMAT_OPEN_RETRY_DELAY >= deadline
                    }) {
                        return Err(error);
                    }
                    tracing::warn!(
                        %camera_id,
                        attempt,
                        %error,
                        "camera format reader open failed; retrying"
                    );
                    last_error = Some(error);
                    std::thread::sleep(CAMERA_FORMAT_OPEN_RETRY_DELAY);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| AppError::message("camera format reader open failed")))
    }

    fn native_nv12_profile(media_type: &IMFMediaType) -> Option<CameraCaptureProfile> {
        let major_type = unsafe { media_type.GetGUID(&MF_MT_MAJOR_TYPE) }.ok()?;
        let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }.ok()?;
        let frame_size = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.ok()?;
        let frame_rate = unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) }.ok()?;
        let width = (frame_size >> 32) as u32;
        let height = frame_size as u32;
        let rate_numerator = (frame_rate >> 32) as u32;
        let rate_denominator = frame_rate as u32;
        if major_type != MFMediaType_Video
            || subtype != MFVideoFormat_NV12
            || width == 0
            || height == 0
            || rate_numerator == 0
            || rate_denominator == 0
        {
            return None;
        }
        Some(CameraCaptureProfile {
            width,
            height,
            fps: rate_numerator
                .saturating_add(rate_denominator / 2)
                .checked_div(rate_denominator)?
                .max(1),
        })
    }

    fn h264_encoder_accepts(
        encoder: &IMFTransform,
        input_type: &IMFMediaType,
        profile: CameraCaptureProfile,
    ) -> AppResult<bool> {
        let output_type = h264_video_type(profile.width, profile.height, profile.fps)?;
        let test_only = MFT_SET_TYPE_TEST_ONLY.0 as u32;
        if unsafe { encoder.SetOutputType(0, &output_type, test_only) }.is_err()
            || unsafe { encoder.SetOutputType(0, &output_type, 0) }.is_err()
        {
            return Ok(false);
        }
        Ok(unsafe { encoder.SetInputType(0, input_type, test_only) }.is_ok())
    }

    fn camera_mode_score(mode: &CameraCaptureProfile, requested: CameraCaptureProfile) -> (u8, u64, u64, u32) {
        let meets_request = mode.width >= requested.width
            && mode.height >= requested.height
            && mode.fps >= requested.fps;
        let aspect_delta = (u64::from(mode.width) * u64::from(requested.height))
            .abs_diff(u64::from(mode.height) * u64::from(requested.width));
        let area_delta = (u64::from(mode.width) * u64::from(mode.height))
            .abs_diff(u64::from(requested.width) * u64::from(requested.height));
        (
            u8::from(!meets_request),
            aspect_delta,
            area_delta,
            mode.fps.abs_diff(requested.fps),
        )
    }

    fn native_nv12_type(
        reader: &IMFSourceReader,
        profile: CameraCaptureProfile,
    ) -> AppResult<IMFMediaType> {
        for index in 0.. {
            let media_type = match unsafe {
                reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, index)
            } {
                Ok(media_type) => media_type,
                Err(_) => break,
            };
            let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }.unwrap_or_default();
            let frame_size = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.unwrap_or_default();
            let frame_rate = unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) }.unwrap_or_default();
            let width = (frame_size >> 32) as u32;
            let height = frame_size as u32;
            let rate_numerator = (frame_rate >> 32) as u32;
            let rate_denominator = frame_rate as u32;
            if subtype == MFVideoFormat_NV12
                && width == profile.width
                && height == profile.height
                && rate_denominator != 0
                && rate_numerator
                    .saturating_add(rate_denominator / 2)
                    .checked_div(rate_denominator)
                    .unwrap_or(0)
                    .max(1)
                    == profile.fps
            {
                return Ok(media_type);
            }
        }
        Err(AppError::message("negotiated native NV12 camera mode is no longer available"))
    }

    fn enum_device_activations(
        attributes: &windows::Win32::Media::MediaFoundation::IMFAttributes,
    ) -> AppResult<Vec<IMFActivate>> {
        let mut raw = ptr::null_mut();
        let mut count = 0_u32;
        unsafe { MFEnumDeviceSources(attributes, &mut raw, &mut count) }.map_err(windows_error)?;
        let mut activates = Vec::with_capacity(count as usize);
        for index in 0..count as usize {
            unsafe {
                if let Some(activate) = ptr::read(raw.add(index)) {
                    activates.push(activate);
                }
            }
        }
        unsafe { CoTaskMemFree(Some(raw.cast())) };
        Ok(activates)
    }

    fn create_attributes(
        initial_size: u32,
    ) -> AppResult<windows::Win32::Media::MediaFoundation::IMFAttributes> {
        let mut attributes = None;
        unsafe { MFCreateAttributes(&mut attributes, initial_size) }.map_err(windows_error)?;
        attributes.ok_or_else(|| AppError::message("Media Foundation did not create attributes"))
    }

    fn activate_h264_encoder() -> AppResult<H264Encoder> {
        let input_type = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_NV12,
        };
        let output_type = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };
        let mut raw = ptr::null_mut();
        let mut count = 0_u32;
        let flags = MFT_ENUM_FLAG(
            MFT_ENUM_FLAG_HARDWARE.0
                | MFT_ENUM_FLAG_SYNCMFT.0
                | MFT_ENUM_FLAG_ASYNCMFT.0
                | MFT_ENUM_FLAG_SORTANDFILTER.0,
        );
        unsafe {
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                flags,
                Some(&input_type),
                Some(&output_type),
                &mut raw,
                &mut count,
            )
        }
        .map_err(windows_error)?;
        let activate = if count == 0 {
            None
        } else {
            unsafe { ptr::read(raw) }
        };
        for index in 1..count as usize {
            unsafe { drop(ptr::read(raw.add(index))) };
        }
        unsafe { CoTaskMemFree(Some(raw.cast())) };
        let activate = activate.ok_or_else(|| {
            AppError::message("no hardware H.264 Media Foundation encoder is available")
        })?;
        let transform: IMFTransform = unsafe { activate.ActivateObject() }.map_err(windows_error)?;
        let attributes = unsafe { transform.GetAttributes() }.map_err(windows_error)?;
        let event_generator = if unsafe { attributes.GetUINT32(&MF_TRANSFORM_ASYNC) }.unwrap_or_default() != 0 {
            unsafe {
                attributes
                    .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                    .map_err(windows_error)?;
            }
            Some(transform.cast().map_err(windows_error)?)
        } else {
            None
        };
        Ok(H264Encoder { transform, event_generator })
    }

    fn h264_video_type(width: u32, height: u32, fps: u32) -> AppResult<IMFMediaType> {
        let media_type = unsafe { MFCreateMediaType() }.map_err(windows_error)?;
        let pixels_per_second = (width as u64)
            .saturating_mul(height as u64)
            .saturating_mul(fps as u64);
        let bitrate = (pixels_per_second / 7).clamp(400_000, 4_000_000) as u32;
        unsafe {
            media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(windows_error)?;
            media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(windows_error)?;
            media_type
                .SetUINT64(&MF_MT_FRAME_SIZE, pack_pair(width, height))
                .map_err(windows_error)?;
            media_type
                .SetUINT64(&MF_MT_FRAME_RATE, pack_pair(fps, 1))
                .map_err(windows_error)?;
            media_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2).map_err(windows_error)?;
            media_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate).map_err(windows_error)?;
            media_type
                .SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, fps)
                .map_err(windows_error)?;
        }
        Ok(media_type)
    }

    fn take_encoded_sample(encoder: &IMFTransform) -> AppResult<Option<(bool, Vec<u8>)>> {
        let output_info = unsafe { encoder.GetOutputStreamInfo(0) }
            .map_err(|error| camera_error("query H.264 encoder output stream", error))?;
        let supplied_sample = if output_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 == 0 {
            let sample = unsafe { MFCreateSample() }
                .map_err(|error| camera_error("allocate H.264 output sample", error))?;
            let buffer = unsafe { MFCreateMemoryBuffer(output_info.cbSize.max(1)) }
                .map_err(|error| camera_error("allocate H.264 output buffer", error))?;
            unsafe { sample.AddBuffer(&buffer) }
                .map_err(|error| camera_error("attach H.264 output buffer", error))?;
            Some(sample)
        } else {
            None
        };
        let mut output = MFT_OUTPUT_DATA_BUFFER::default();
        output.dwStreamID = 0;
        output.pSample = ManuallyDrop::new(supplied_sample);
        let mut status = 0_u32;
        let result = unsafe { encoder.ProcessOutput(0, std::slice::from_mut(&mut output), &mut status) };
        let sample = unsafe { ManuallyDrop::take(&mut output.pSample) };
        match result {
            Ok(()) => {
                let Some(sample) = sample else { return Ok(None); };
                let bytes = sample_bytes(&sample)?;
                if bytes.is_empty() {
                    return Ok(None);
                }
                let mut annex_b = h264_to_annex_b(&bytes);
                let keyframe = contains_idr(&annex_b)
                    || unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) != 0;
                if keyframe && !contains_parameter_sets(&annex_b) {
                    if let Ok(media_type) = unsafe { encoder.GetOutputCurrentType(0) } {
                        let mut parameter_sets = h264_parameter_sets(&media_type);
                        if !parameter_sets.is_empty() {
                            parameter_sets.extend_from_slice(&annex_b);
                            annex_b = parameter_sets;
                        }
                    }
                }
                if keyframe && !contains_parameter_sets(&annex_b) {
                    return Err(AppError::message(
                        "H.264 encoder produced a keyframe without SPS/PPS",
                    ));
                }
                Ok(Some((keyframe, annex_b)))
            }
            Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(None),
            Err(error) => Err(camera_error("read H.264 encoder output", error)),
        }
    }

    fn sample_bytes(sample: &windows::Win32::Media::MediaFoundation::IMFSample) -> AppResult<Vec<u8>> {
        let buffer: IMFMediaBuffer = unsafe { sample.ConvertToContiguousBuffer() }
            .map_err(|error| camera_error("read encoded H.264 sample", error))?;
        let mut data = ptr::null_mut();
        let mut length = 0_u32;
        unsafe { buffer.Lock(&mut data, None, Some(&mut length)) }
            .map_err(|error| camera_error("lock encoded H.264 sample", error))?;
        let bytes = unsafe { std::slice::from_raw_parts(data, length as usize) }.to_vec();
        unsafe { buffer.Unlock() }
            .map_err(|error| camera_error("unlock encoded H.264 sample", error))?;
        Ok(bytes)
    }

    fn get_attribute_string(
        attributes: &impl AttributeString,
        key: &windows::core::GUID,
    ) -> AppResult<String> {
        let length = attributes.get_string_length(key)?;
        let mut value = vec![0_u16; length as usize + 1];
        attributes.get_string(key, &mut value)?;
        Ok(String::from_utf16_lossy(&value[..length as usize]))
    }

    trait AttributeString {
        fn get_string_length(&self, key: &windows::core::GUID) -> AppResult<u32>;
        fn get_string(&self, key: &windows::core::GUID, value: &mut [u16]) -> AppResult<()>;
    }

    impl AttributeString for IMFActivate {
        fn get_string_length(&self, key: &windows::core::GUID) -> AppResult<u32> {
            unsafe { self.GetStringLength(key) }.map_err(windows_error)
        }

        fn get_string(&self, key: &windows::core::GUID, value: &mut [u16]) -> AppResult<()> {
            unsafe { self.GetString(key, value, None) }.map_err(windows_error)
        }
    }

    fn h264_parameter_sets(media_type: &IMFMediaType) -> Vec<u8> {
        let Ok(length) = (unsafe { media_type.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) }) else {
            return Vec::new();
        };
        let mut configuration = vec![0_u8; length as usize];
        if unsafe { media_type.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut configuration, None) }.is_err() {
            return Vec::new();
        }
        avcc_parameter_sets_to_annex_b(&configuration)
    }

    fn h264_to_annex_b(bytes: &[u8]) -> Vec<u8> {
        if has_start_code(bytes) {
            return bytes.to_vec();
        }
        for length_size in [4_usize, 2, 1] {
            let mut input = bytes;
            let mut annex_b = Vec::with_capacity(bytes.len() + 16);
            let mut valid = false;
            while input.len() >= length_size {
                let length = input[..length_size]
                    .iter()
                    .fold(0_usize, |value, byte| value << 8 | *byte as usize);
                input = &input[length_size..];
                if length == 0 || input.len() < length {
                    valid = false;
                    break;
                }
                valid = true;
                annex_b.extend_from_slice(&[0, 0, 0, 1]);
                annex_b.extend_from_slice(&input[..length]);
                input = &input[length..];
            }
            if valid && input.is_empty() {
                return annex_b;
            }
        }
        bytes.to_vec()
    }

    fn avcc_parameter_sets_to_annex_b(configuration: &[u8]) -> Vec<u8> {
        if configuration.len() < 7 || configuration[0] != 1 {
            return Vec::new();
        }
        let mut offset = 5;
        let sps_count = (configuration[offset] & 0x1f) as usize;
        offset += 1;
        let mut annex_b = Vec::new();
        for _ in 0..sps_count {
            if offset + 2 > configuration.len() {
                return Vec::new();
            }
            let length = u16::from_be_bytes([configuration[offset], configuration[offset + 1]]) as usize;
            offset += 2;
            if offset + length > configuration.len() {
                return Vec::new();
            }
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(&configuration[offset..offset + length]);
            offset += length;
        }
        if offset >= configuration.len() {
            return annex_b;
        }
        let pps_count = configuration[offset] as usize;
        offset += 1;
        for _ in 0..pps_count {
            if offset + 2 > configuration.len() {
                return Vec::new();
            }
            let length = u16::from_be_bytes([configuration[offset], configuration[offset + 1]]) as usize;
            offset += 2;
            if offset + length > configuration.len() {
                return Vec::new();
            }
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(&configuration[offset..offset + length]);
            offset += length;
        }
        annex_b
    }

    fn contains_idr(bytes: &[u8]) -> bool {
        nal_unit_types(bytes).contains(&5)
    }

    fn contains_parameter_sets(bytes: &[u8]) -> bool {
        let mut sps = false;
        let mut pps = false;
        for nal_type in nal_unit_types(bytes) {
            sps |= nal_type == 7;
            pps |= nal_type == 8;
        }
        sps && pps
    }

    fn nal_unit_types(bytes: &[u8]) -> Vec<u8> {
        let mut types = Vec::new();
        let mut index = 0;
        while index + 3 <= bytes.len() {
            let start_code_length = if bytes[index..].starts_with(&[0, 0, 0, 1]) {
                4
            } else if bytes[index..].starts_with(&[0, 0, 1]) {
                3
            } else {
                index += 1;
                continue;
            };
            if let Some(byte) = bytes.get(index + start_code_length) {
                types.push(byte & 0x1f);
            }
            index += start_code_length;
        }
        types
    }

    fn has_start_code(bytes: &[u8]) -> bool {
        bytes.windows(3).any(|window| window == [0, 0, 1])
            || bytes.windows(4).any(|window| window == [0, 0, 0, 1])
    }

    const fn pack_pair(first: u32, second: u32) -> u64 {
        ((first as u64) << 32) | second as u64
    }

    fn camera_error(operation: &str, error: impl Into<WindowsError>) -> AppError {
        AppError::message(format!("{operation}: {}", error.into()))
    }

    fn windows_error(error: impl Into<WindowsError>) -> AppError {
        AppError::message(error.into().to_string())
    }

    #[cfg(test)]
    mod tests {
        use std::{
            collections::HashMap,
            sync::{
                atomic::{AtomicUsize, Ordering},
                Arc,
            },
            time::{Duration, Instant},
        };

        use super::{
            avcc_parameter_sets_to_annex_b, capture, h264_to_annex_b, list_devices,
            list_devices_v2, negotiate, CameraCaptureRequest, AtomicBool,
        };

        #[test]
        fn converts_length_prefixed_h264_access_unit_to_annex_b() {
            assert_eq!(
                h264_to_annex_b(&[0, 0, 0, 2, 0x67, 0x42, 0, 0, 0, 2, 0x68, 0xce]),
                vec![0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xce],
            );
        }

        #[test]
        fn converts_avcc_parameter_sets_to_annex_b() {
            assert_eq!(
                avcc_parameter_sets_to_annex_b(&[
                    1, 0x42, 0, 0x1e, 0xff, 0xe1, 0, 2, 0x67, 0x42, 1, 0, 2, 0x68,
                    0xce,
                ]),
                vec![0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xce],
            );
        }

        #[test]
        #[ignore = "requires a connected Windows camera and hardware H.264 encoder"]
        fn native_camera_v2_enumeration_smoke() {
            let started_at = Instant::now();
            let cameras = list_devices_v2(&HashMap::new())
                .expect("enumerate cameras with verified H.264 capabilities");
            assert!(!cameras.is_empty(), "no encodable camera was enumerated");
            for camera in cameras {
                let capabilities = camera
                    .capabilities
                    .expect("V2 camera must include capabilities");
                assert!(!capabilities.resolutions.is_empty());
                assert!(capabilities.resolutions.len() <= super::super::MAX_CAMERA_RESOLUTION_TIERS);
                assert!(capabilities.resolutions.iter().all(|resolution| {
                    resolution.width > 0
                        && resolution.height > 0
                        && !resolution.fps.is_empty()
                }));
            }
            eprintln!("V2 camera enumeration completed in {:?}", started_at.elapsed());
        }

        #[test]
        #[ignore = "requires a connected Windows camera and hardware H.264 encoder"]
        fn native_camera_pipeline_smoke() {
            let camera = list_devices()
                .expect("enumerate cameras")
                .into_iter()
                .next()
                .expect("at least one camera must be connected");
            let profile = negotiate(&camera.camera_id, 960, 540, 15)
                .expect("negotiate native camera profile");
            let cancelled = Arc::new(AtomicBool::new(false));
            let watchdog_cancelled = cancelled.clone();
            let watchdog = std::thread::spawn(move || {
                for _ in 0..120 {
                    if watchdog_cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                watchdog_cancelled.store(true, Ordering::Release);
            });
            let frame_count = Arc::new(AtomicUsize::new(0));
            let emitted_frames = frame_count.clone();
            let stop_after_frames = cancelled.clone();
            let saw_keyframe = Arc::new(AtomicBool::new(false));
            let emitted_keyframe = saw_keyframe.clone();

            let result = capture(
                CameraCaptureRequest {
                    session_id: "native-camera-smoke".to_owned(),
                    generation: 1,
                    camera_id: camera.camera_id,
                    width: profile.width,
                    height: profile.height,
                    fps: profile.fps,
                },
                cancelled,
                move |keyframe, payload| {
                    assert!(!payload.is_empty(), "H.264 output must not be empty");
                    assert!(super::has_start_code(&payload), "H.264 output must use Annex B framing");
                    if keyframe {
                        emitted_keyframe.store(true, Ordering::Release);
                    }
                    if emitted_frames.fetch_add(1, Ordering::AcqRel) + 1 >= 30 {
                        stop_after_frames.store(true, Ordering::Release);
                    }
                },
            );
            watchdog.join().expect("camera watchdog panicked");

            result.expect("native camera capture failed");
            assert!(
                frame_count.load(Ordering::Acquire) >= 30,
                "native camera did not produce 30 H.264 frames"
            );
            assert!(
                saw_keyframe.load(Ordering::Acquire),
                "native camera did not produce a H.264 keyframe"
            );
        }

        #[test]
        #[ignore = "requires a connected Windows camera"]
        fn native_camera_source_smoke() {
            let camera = list_devices()
                .expect("enumerate cameras")
                .into_iter()
                .next()
                .expect("at least one camera must be connected");
            let _com = super::ComApartment::new().expect("initialize COM");
            let _media_foundation = super::MediaFoundation::new().expect("initialize Media Foundation");
            let activate = super::find_device(&camera.camera_id).expect("find camera");
            let source = super::MediaSourceSession(
                unsafe { activate.ActivateObject::<super::IMFMediaSource>() }
                    .expect("activate camera source"),
            );
            let reader = unsafe { super::MFCreateSourceReaderFromMediaSource(&source.0, None) }
                .expect("create camera source reader");
            unsafe {
                reader
                    .SetStreamSelection(super::MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                    .expect("disable non-video streams");
                reader
                    .SetStreamSelection(super::MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
                    .expect("enable video stream");
            }

            let mut native_types = Vec::new();
            for index in 0.. {
                let media_type = match unsafe {
                    reader.GetNativeMediaType(super::MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, index)
                } {
                    Ok(media_type) => media_type,
                    Err(_) => break,
                };
                let frame_size = unsafe { media_type.GetUINT64(&super::MF_MT_FRAME_SIZE) }
                    .expect("native type frame size");
                let frame_rate = unsafe { media_type.GetUINT64(&super::MF_MT_FRAME_RATE) }
                    .expect("native type frame rate");
                let subtype = unsafe { media_type.GetGUID(&super::MF_MT_SUBTYPE) }
                    .expect("native type subtype");
                native_types.push((media_type, frame_size, frame_rate, subtype));
            }
            assert!(!native_types.is_empty(), "camera exposes no native video types");
            let profile = super::select_camera_profile(&camera.camera_id, 960, 540, 15)
                .expect("negotiate native camera profile");
            let (native_type, _, _, _) = native_types
                .into_iter()
                .find(|(_, frame_size, frame_rate, subtype)| {
                    *subtype == super::MFVideoFormat_NV12
                        && (*frame_size >> 32) as u32 == profile.width
                        && *frame_size as u32 == profile.height
                        && (*frame_rate >> 32) as u32 / (*frame_rate as u32).max(1) == profile.fps
                })
                .expect("negotiated native camera type");
            unsafe {
                reader
                    .SetCurrentMediaType(
                        super::MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        None,
                        &native_type,
                    )
                    .expect("select first native camera type");
            }
            let mut sample = None;
            let mut flags = 0_u32;
            for _ in 0..30 {
                unsafe {
                    reader
                        .ReadSample(
                            super::MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                            0,
                            None,
                            Some(&mut flags),
                            None,
                            Some(&mut sample),
                        )
                        .expect("read native camera frame");
                }
                if sample.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(sample.is_some(), "camera returned no native frame; flags=0x{flags:08X}");
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::{collections::HashMap, sync::{atomic::AtomicBool, Arc}};

    use crate::{
        error::{AppError, AppResult},
        protocol::{CameraCapabilities, CameraEntry},
    };

    use super::{CameraCaptureProfile, CameraCaptureRequest};

    pub(super) fn list_devices() -> AppResult<Vec<CameraEntry>> {
        Ok(Vec::new())
    }

    pub(super) fn list_devices_v2(
        _: &HashMap<String, CameraCapabilities>,
    ) -> AppResult<Vec<CameraEntry>> {
        Ok(Vec::new())
    }

    pub(super) fn negotiate(
        _: &str,
        _: u32,
        _: u32,
        _: u32,
    ) -> AppResult<CameraCaptureProfile> {
        Err(AppError::message("native camera capture is currently available on Windows only"))
    }

    pub(super) fn negotiate_exact(
        _: &str,
        _: u32,
        _: u32,
        _: u32,
    ) -> AppResult<CameraCaptureProfile> {
        Err(AppError::message("native camera capture is currently available on Windows only"))
    }

    pub(super) fn capture(
        _: CameraCaptureRequest,
        _: Arc<AtomicBool>,
        _: impl FnMut(bool, Vec<u8>),
    ) -> AppResult<()> {
        Err(AppError::message("native camera capture is currently available on Windows only"))
    }
}
