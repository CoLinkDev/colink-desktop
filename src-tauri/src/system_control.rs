use std::{
    io,
    process::{Command, Stdio},
};

use crate::protocol::SystemControlAction;

pub fn execute_system_control(action: SystemControlAction) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    if action == SystemControlAction::Sleep {
        return suspend_windows();
    }

    let mut command = system_command(action);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
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
    }
    command
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
fn system_command(_action: SystemControlAction) -> Command {
    Command::new("false")
}

#[cfg(test)]
mod tests {
    use crate::protocol::SystemControlAction;

    #[test]
    fn recognizes_only_documented_actions() {
        assert_eq!(SystemControlAction::parse("sleep"), Some(SystemControlAction::Sleep));
        assert_eq!(SystemControlAction::parse("shutdown"), Some(SystemControlAction::Shutdown));
        assert_eq!(SystemControlAction::parse("lock"), Some(SystemControlAction::Lock));
        assert_eq!(SystemControlAction::parse("restart"), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_sleep_request_never_requests_hibernation() {
        assert_eq!(super::windows_sleep_request(), (false, false, false));
    }
}
