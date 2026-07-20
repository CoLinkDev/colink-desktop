use std::{
    io,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    process::{Command, Stdio},
};

use if_addrs::{get_if_addrs, IfAddr};

use crate::protocol::{is_valid_wake_on_lan_mac, SystemControlAction, SystemControlResultPayload};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemControlExecution {
    Executed,
    Ignored,
}

pub fn execute_system_control(
    action: SystemControlAction,
    volume: Option<i32>,
    target_mac: Option<&str>,
) -> io::Result<SystemControlExecution> {
    if !action.accepts_volume(volume) || !action.accepts_target_mac(target_mac) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid system control payload",
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
        SystemControlAction::WakeOnLan => {
            return send_wake_on_lan_packet(target_mac.expect("validated wake-on-lan payload"));
        }
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

fn send_wake_on_lan_packet(target_mac: &str) -> io::Result<SystemControlExecution> {
    let packet = wake_on_lan_magic_packet(target_mac).expect("validated wake-on-lan MAC address");
    let destinations = wake_on_lan_broadcast_addresses()?;
    if destinations.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "no active IPv4 broadcast interface",
        ));
    }

    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;

    let mut sent = false;
    let mut last_error = None;
    for destination in destinations {
        match socket.send_to(&packet, SocketAddrV4::new(destination, 9)) {
            Ok(_) => sent = true,
            Err(error) => last_error = Some(error),
        }
    }
    if sent {
        Ok(SystemControlExecution::Executed)
    } else {
        Err(last_error.unwrap_or_else(|| {
            io::Error::other("failed to send Wake-on-LAN packet")
        }))
    }
}

fn wake_on_lan_broadcast_addresses() -> io::Result<Vec<Ipv4Addr>> {
    let mut addresses = get_if_addrs()?
        .into_iter()
        .filter(|interface| {
            interface.is_oper_up() && !interface.is_loopback() && !interface.is_p2p()
        })
        .filter_map(|interface| match interface.addr {
            IfAddr::V4(address) if !address.ip.is_unspecified() && !address.ip.is_link_local() => {
                address.broadcast
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    Ok(addresses)
}

fn wake_on_lan_magic_packet(target_mac: &str) -> Option<[u8; 102]> {
    if !is_valid_wake_on_lan_mac(target_mac) {
        return None;
    }
    let mut mac = [0u8; 6];
    for (index, part) in target_mac.split(':').enumerate() {
        mac[index] = u8::from_str_radix(part, 16).ok()?;
    }
    let mut packet = [0xff; 102];
    for index in 0..16 {
        let start = 6 + (index * mac.len());
        packet[start..start + mac.len()].copy_from_slice(&mac);
    }
    Some(packet)
}

pub fn query_system_control(fields: &[String]) -> io::Result<SystemControlResultPayload> {
    let queries_volume = fields.iter().any(|field| field == "volume");
    let queries_muted = fields.iter().any(|field| field == "muted");
    let queries_playback = fields.iter().any(|field| field == "playback");

    let audio_state = if queries_volume || queries_muted {
        query_system_audio_state()?
    } else {
        None
    };
    let playback = if queries_playback {
        query_media_playback_state()?
    } else {
        None
    };

    Ok(SystemControlResultPayload {
        volume: queries_volume.then_some(audio_state.map(|(volume, _)| volume)),
        muted: queries_muted.then_some(audio_state.map(|(_, muted)| muted)),
        playback: queries_playback.then_some(playback),
    })
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
        _ => unreachable!("media, audio, and Wake-on-LAN actions are handled before system commands"),
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
        _ => unreachable!("media, audio, and Wake-on-LAN actions are handled before system commands"),
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
        _ => unreachable!("media, audio, and Wake-on-LAN actions are handled before system commands"),
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
fn query_media_playback_state() -> io::Result<Option<String>> {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };

    let _runtime = WindowsRuntimeGuard::initialize()?;
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .map_err(windows_error)?
        .get()
        .map_err(windows_error)?;
    let Ok(session) = manager.GetCurrentSession() else {
        return Ok(None);
    };
    let Ok(playback_info) = session.GetPlaybackInfo() else {
        return Ok(None);
    };
    let Ok(status) = playback_info.PlaybackStatus() else {
        return Ok(None);
    };
    let state = if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing {
        Some("playing")
    } else if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused {
        Some("paused")
    } else if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Stopped {
        Some("stopped")
    } else {
        None
    };
    Ok(state.map(str::to_string))
}

#[cfg(not(windows))]
fn query_media_playback_state() -> io::Result<Option<String>> {
    Ok(None)
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
fn query_system_audio_state() -> io::Result<Option<(i32, bool)>> {
    let _runtime = WindowsRuntimeGuard::initialize()?;
    let endpoint = default_audio_endpoint_volume()?;
    let volume = unsafe {
        endpoint
            .GetMasterVolumeLevelScalar()
            .map_err(windows_error)?
    };
    let muted = unsafe { endpoint.GetMute().map_err(windows_error)? }.as_bool();
    Ok(Some(((volume * 100.0).round().clamp(0.0, 100.0) as i32, muted)))
}

#[cfg(not(windows))]
fn query_system_audio_state() -> io::Result<Option<(i32, bool)>> {
    Ok(None)
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
    use crate::protocol::{SystemControlAction, SystemControlResultPayload};

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
        assert_eq!(
            SystemControlAction::parse("wake-on-lan"),
            Some(SystemControlAction::WakeOnLan)
        );
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

    #[test]
    fn validates_wake_on_lan_mac_and_magic_packet() {
        let mac = "01:23:45:67:89:ab";
        assert!(SystemControlAction::WakeOnLan.accepts_target_mac(Some(mac)));
        assert!(!SystemControlAction::WakeOnLan.accepts_target_mac(Some("01:23:45:67:89")));
        assert!(!SystemControlAction::Sleep.accepts_target_mac(Some(mac)));

        let packet = super::wake_on_lan_magic_packet(mac).expect("valid magic packet");
        assert_eq!(&packet[..6], &[0xff; 6]);
        assert_eq!(&packet[6..12], &[1, 35, 69, 103, 137, 171]);
        assert_eq!(&packet[96..], &[1, 35, 69, 103, 137, 171]);
    }

    #[test]
    fn serializes_unavailable_requested_state_as_null() {
        let payload = SystemControlResultPayload {
            volume: Some(None),
            muted: Some(Some(true)),
            playback: None,
        };

        assert_eq!(
            serde_json::to_string(&payload).expect("serialize system state"),
            r#"{"volume":null,"muted":true}"#,
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_sleep_request_never_requests_hibernation() {
        assert_eq!(super::windows_sleep_request(), (false, false, false));
    }
}
