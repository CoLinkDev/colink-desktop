use std::{
    io,
    process::{Command, Stdio},
};

use crate::protocol::SystemControlAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemControlExecution {
    Executed,
    Ignored,
}

pub fn execute_system_control(
    action: SystemControlAction,
    volume: Option<i32>,
) -> io::Result<SystemControlExecution> {
    if !action.accepts_volume(volume) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid system control volume",
        ));
    }

    match action {
        SystemControlAction::Play
        | SystemControlAction::Pause
        | SystemControlAction::Next
        | SystemControlAction::Previous => return execute_media_control(action),
        SystemControlAction::SetVolume => {
            return set_system_volume(volume.expect("validated set-volume payload"));
        }
        SystemControlAction::Mute => return mute_system_audio(),
        SystemControlAction::Sleep | SystemControlAction::Shutdown | SystemControlAction::Lock => {}
    }

    #[cfg(target_os = "windows")]
    if action == SystemControlAction::Sleep {
        return suspend_windows().map(|_| SystemControlExecution::Executed);
    }

    let mut command = system_command(action);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| SystemControlExecution::Executed)
}

#[cfg(target_os = "windows")]
fn suspend_windows() -> io::Result<()> {
    let (hibernate, force, wakeup_events_disabled) = windows_sleep_request();
    if unsafe {
        windows::Win32::System::Power::SetSuspendState(
            hibernate,
            force,
            wakeup_events_disabled,
        )
    } {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "windows")]
const fn windows_sleep_request() -> (bool, bool, bool) {
    (false, false, false)
}

#[cfg(target_os = "windows")]
fn system_command(action: SystemControlAction) -> Command {
    let command = match action {
        SystemControlAction::Shutdown => {
            let mut command = Command::new("shutdown.exe");
            command.args(["/s", "/t", "0"]);
            command
        }
        SystemControlAction::Lock => {
            let mut command = Command::new("rundll32.exe");
            command.arg("user32.dll,LockWorkStation");
            command
        }
        SystemControlAction::Sleep => unreachable!("sleep is handled through SetSuspendState"),
        _ => unreachable!("media and audio actions are handled before system commands"),
    };
    command
}

#[cfg(target_os = "macos")]
fn system_command(action: SystemControlAction) -> Command {
    let command = match action {
        SystemControlAction::Sleep => {
            let mut command = Command::new("pmset");
            command.args(["sleepnow"]);
            command
        }
        SystemControlAction::Shutdown => {
            let mut command = Command::new("/sbin/shutdown");
            command.args(["-h", "now"]);
            command
        }
        SystemControlAction::Lock => {
            let mut command = Command::new(
                "/System/Library/CoreServices/Menu Extras/User.menu/Contents/Resources/CGSession",
            );
            command.arg("-suspend");
            command
        }
        _ => unreachable!("media and audio actions are handled before system commands"),
    };
    command
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_command(action: SystemControlAction) -> Command {
    let mut command = Command::new("loginctl");
    match action {
        SystemControlAction::Sleep => {
            command.arg("suspend");
        }
        SystemControlAction::Shutdown => {
            command.arg("poweroff");
        }
        SystemControlAction::Lock => {
            command.arg("lock-session");
        }
        _ => unreachable!("media and audio actions are handled before system commands"),
    }
    command
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
fn system_command(_action: SystemControlAction) -> Command {
    Command::new("false")
}

#[cfg(windows)]
fn execute_media_control(action: SystemControlAction) -> io::Result<SystemControlExecution> {
    let _runtime = WindowsRuntimeGuard::initialize()?;
    let manager = windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .map_err(windows_error)?
        .get()
        .map_err(windows_error)?;
    let Ok(session) = manager.GetCurrentSession() else {
        return Ok(SystemControlExecution::Ignored);
    };
    let succeeded = match action {
        SystemControlAction::Play => session.TryPlayAsync(),
        SystemControlAction::Pause => session.TryPauseAsync(),
        SystemControlAction::Next => session.TrySkipNextAsync(),
        SystemControlAction::Previous => session.TrySkipPreviousAsync(),
        _ => unreachable!("only media transport actions reach this function"),
    }
    .map_err(windows_error)?
    .get()
    .map_err(windows_error)?;
    if succeeded {
        Ok(SystemControlExecution::Executed)
    } else {
        Ok(SystemControlExecution::Ignored)
    }
}

#[cfg(not(windows))]
fn execute_media_control(_action: SystemControlAction) -> io::Result<SystemControlExecution> {
    Ok(SystemControlExecution::Ignored)
}

#[cfg(windows)]
fn set_system_volume(volume: i32) -> io::Result<SystemControlExecution> {
    let _runtime = WindowsRuntimeGuard::initialize()?;
    let endpoint = default_audio_endpoint_volume()?;
    unsafe {
        endpoint
            .SetMasterVolumeLevelScalar(volume as f32 / 100.0, std::ptr::null())
            .map_err(windows_error)?;
        if volume > 0 {
            endpoint.SetMute(false, std::ptr::null()).map_err(windows_error)?;
        }
    }
    Ok(SystemControlExecution::Executed)
}

#[cfg(not(windows))]
fn set_system_volume(_volume: i32) -> io::Result<SystemControlExecution> {
    Ok(SystemControlExecution::Ignored)
}

#[cfg(windows)]
fn mute_system_audio() -> io::Result<SystemControlExecution> {
    let _runtime = WindowsRuntimeGuard::initialize()?;
    let endpoint = default_audio_endpoint_volume()?;
    unsafe {
        endpoint.SetMute(true, std::ptr::null()).map_err(windows_error)?;
    }
    Ok(SystemControlExecution::Executed)
}

#[cfg(not(windows))]
fn mute_system_audio() -> io::Result<SystemControlExecution> {
    Ok(SystemControlExecution::Ignored)
}

#[cfg(windows)]
fn default_audio_endpoint_volume(
) -> io::Result<windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume> {
    use windows::Win32::{
        Media::Audio::{
            Endpoints::IAudioEndpointVolume, eMultimedia, eRender, IMMDeviceEnumerator,
            MMDeviceEnumerator,
        },
        System::Com::{CoCreateInstance, CLSCTX_ALL},
    };

    let enumerator: IMMDeviceEnumerator = unsafe {
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(windows_error)?
    };
    let device = unsafe {
        enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .map_err(windows_error)?
    };
    unsafe {
        device
            .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            .map_err(windows_error)
    }
}

#[cfg(windows)]
struct WindowsRuntimeGuard;

#[cfg(windows)]
impl WindowsRuntimeGuard {
    fn initialize() -> io::Result<Self> {
        unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
            )
            .map_err(windows_error)?;
        }
        Ok(Self)
    }
}

#[cfg(windows)]
impl Drop for WindowsRuntimeGuard {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::WinRT::RoUninitialize();
        }
    }
}

#[cfg(windows)]
fn windows_error(error: windows::core::Error) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use crate::protocol::SystemControlAction;

    #[test]
    fn recognizes_documented_actions() {
        assert_eq!(SystemControlAction::parse("sleep"), Some(SystemControlAction::Sleep));
        assert_eq!(SystemControlAction::parse("shutdown"), Some(SystemControlAction::Shutdown));
        assert_eq!(SystemControlAction::parse("lock"), Some(SystemControlAction::Lock));
        assert_eq!(SystemControlAction::parse("play"), Some(SystemControlAction::Play));
        assert_eq!(SystemControlAction::parse("pause"), Some(SystemControlAction::Pause));
        assert_eq!(SystemControlAction::parse("next"), Some(SystemControlAction::Next));
        assert_eq!(
            SystemControlAction::parse("previous"),
            Some(SystemControlAction::Previous)
        );
        assert_eq!(
            SystemControlAction::parse("set-volume"),
            Some(SystemControlAction::SetVolume)
        );
        assert_eq!(SystemControlAction::parse("mute"), Some(SystemControlAction::Mute));
        assert_eq!(SystemControlAction::parse("restart"), None);
    }

    #[test]
    fn validates_volume_only_for_set_volume() {
        assert!(SystemControlAction::SetVolume.accepts_volume(Some(0)));
        assert!(SystemControlAction::SetVolume.accepts_volume(Some(100)));
        assert!(!SystemControlAction::SetVolume.accepts_volume(None));
        assert!(!SystemControlAction::SetVolume.accepts_volume(Some(101)));
        assert!(SystemControlAction::Mute.accepts_volume(None));
        assert!(!SystemControlAction::Mute.accepts_volume(Some(20)));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_sleep_request_never_requests_hibernation() {
        assert_eq!(super::windows_sleep_request(), (false, false, false));
    }
}
