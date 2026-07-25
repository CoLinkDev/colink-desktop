use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

use tokio::sync::mpsc;

use crate::{
    error::{AppError, AppResult},
    protocol::CameraEntry,
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

#[derive(Clone, Copy)]
pub(super) struct CameraCaptureProfile {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Clone)]
pub(super) struct CameraCaptureService {
    state: Arc<Mutex<CaptureState>>,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
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
        }
    }

    pub(super) fn list_devices(&self) -> AppResult<Vec<CameraEntry>> {
        platform::list_devices()
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
    use std::sync::{atomic::AtomicBool, Arc};

    use super::{CameraCaptureRequest, CaptureState};

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
}

#[cfg(windows)]
mod platform {
    use std::{
        mem::ManuallyDrop,
        ptr,
        sync::{atomic::{AtomicBool, Ordering}, Arc},
        time::Duration,
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
                MFSampleExtension_CleanPoint, MF_SOURCE_READER_ALL_STREAMS,
                MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM,
                METransformDrainComplete, METransformHaveOutput, METransformNeedInput, MFSTARTUP_FULL, MF_VERSION,
                MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_NV12,
            },
            System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED},
        },
    };

    use crate::{error::{AppError, AppResult}, protocol::CameraEntry};

    use super::{CameraCaptureProfile, CameraCaptureRequest};

    const HNS_PER_SECOND: u64 = 10_000_000;

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

    struct NativeCameraMode {
        width: u32,
        height: u32,
        fps: u32,
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
        let activate = find_device(camera_id)?;
        let source = MediaSourceSession(
            unsafe { activate.ActivateObject::<IMFMediaSource>() }
                .map_err(|error| camera_error("open camera for format negotiation", error))?,
        );
        let reader = unsafe { MFCreateSourceReaderFromMediaSource(&source.0, None) }
            .map_err(|error| camera_error("create camera format reader", error))?;
        let mut modes = Vec::new();
        for index in 0.. {
            let media_type = match unsafe {
                reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, index)
            } {
                Ok(media_type) => media_type,
                Err(_) => break,
            };
            let major_type = unsafe { media_type.GetGUID(&MF_MT_MAJOR_TYPE) }.unwrap_or_default();
            let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }.unwrap_or_default();
            let frame_size = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.unwrap_or_default();
            let frame_rate = unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) }.unwrap_or_default();
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
                continue;
            }
            modes.push(NativeCameraMode {
                width,
                height,
                fps: (rate_numerator / rate_denominator).max(1),
            });
        }
        let requested = CameraCaptureProfile {
            width: requested_width,
            height: requested_height,
            fps: requested_fps,
        };
        let mode = modes
            .into_iter()
            .min_by_key(|mode| camera_mode_score(mode, requested))
            .ok_or_else(|| AppError::message("camera has no native NV12 capture mode"))?;
        Ok(CameraCaptureProfile {
            width: mode.width,
            height: mode.height,
            fps: mode.fps,
        })
    }

    fn camera_mode_score(mode: &NativeCameraMode, requested: CameraCaptureProfile) -> (u8, u64, u64, u32) {
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
                && (rate_numerator / rate_denominator).max(1) == profile.fps
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
            sync::{
                atomic::{AtomicUsize, Ordering},
                Arc,
            },
            time::Duration,
        };

        use super::{
            avcc_parameter_sets_to_annex_b, capture, h264_to_annex_b, list_devices, negotiate,
            CameraCaptureRequest, AtomicBool,
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
    use std::{sync::{atomic::AtomicBool, Arc}};

    use crate::{error::{AppError, AppResult}, protocol::CameraEntry};

    use super::{CameraCaptureProfile, CameraCaptureRequest};

    pub(super) fn list_devices() -> AppResult<Vec<CameraEntry>> {
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

    pub(super) fn capture(
        _: CameraCaptureRequest,
        _: Arc<AtomicBool>,
        _: impl FnMut(bool, Vec<u8>),
    ) -> AppResult<()> {
        Err(AppError::message("native camera capture is currently available on Windows only"))
    }
}
