//! systemd unit generation — how zero-idle and scheduled wake actually happen on a VPS.
//!
//! Socket activation is the mechanism behind the headline "0 MB at idle": systemd
//! owns the listening socket, and only spawns `engramd` when a connection arrives.
//! Between requests there is no Engram process at all. A separate timer arms the next
//! scheduled fire, waking the core just in time. These are pure string generators so
//! they can be unit-tested and written by the deploy command.

/// The `.socket` + `.service` pair for socket-activated, zero-idle operation.
/// `exec` is the absolute path to the `engramd` binary; `port` is the TCP port.
pub fn socket_activation(exec: &str, port: u16) -> (String, String) {
    let socket = format!(
        "[Unit]\n\
         Description=Engram core socket (zero-idle activation)\n\n\
         [Socket]\n\
         ListenStream={port}\n\
         # Hand the accepted connection to a freshly-spawned engramd.\n\
         Accept=no\n\n\
         [Install]\n\
         WantedBy=sockets.target\n"
    );
    let service = format!(
        "[Unit]\n\
         Description=Engram core (socket-activated)\n\
         Requires=engram.socket\n\
         After=engram.socket\n\n\
         [Service]\n\
         Type=notify\n\
         ExecStart={exec}\n\
         # Hardening: minimal privileges for a self-modifying agent.\n\
         DynamicUser=yes\n\
         NoNewPrivileges=yes\n\
         ProtectSystem=strict\n\
         ProtectHome=yes\n\
         PrivateTmp=yes\n\
         StateDirectory=engram\n\
         Environment=ENGRAM_HOME=/var/lib/engram\n"
    );
    (socket, service)
}

/// A one-shot `.service` + `.timer` that wakes the core at `on_calendar`
/// (a systemd `OnCalendar=` expression, e.g. "*-*-* 09:00:00").
pub fn wake_timer(exec: &str, on_calendar: &str) -> (String, String) {
    let service = format!(
        "[Unit]\n\
         Description=Engram scheduled wake\n\n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={exec} --run-due\n\
         Environment=ENGRAM_HOME=/var/lib/engram\n"
    );
    let timer = format!(
        "[Unit]\n\
         Description=Engram scheduled wake timer\n\n\
         [Timer]\n\
         OnCalendar={on_calendar}\n\
         Persistent=true\n\n\
         [Install]\n\
         WantedBy=timers.target\n"
    );
    (service, timer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_unit_has_listen_and_activation() {
        let (socket, service) = socket_activation("/usr/local/bin/engramd", 8088);
        assert!(socket.contains("ListenStream=8088"));
        assert!(service.contains("Requires=engram.socket"));
        assert!(service.contains("NoNewPrivileges=yes"));
    }

    #[test]
    fn timer_has_calendar_and_persistent() {
        let (_svc, timer) = wake_timer("/usr/local/bin/engramd", "*-*-* 09:00:00");
        assert!(timer.contains("OnCalendar=*-*-* 09:00:00"));
        assert!(timer.contains("Persistent=true"));
    }
}
