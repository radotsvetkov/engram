//! systemd unit generation - how zero-idle and scheduled wake actually happen on a VPS.
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
         # Bind loopback by default - an exposed agent must be authenticated. To reach it from\n\
         # the network, front it with a reverse proxy (TLS + the API token) or change this to\n\
         # ListenStream={port} AND set ENGRAM_API_TOKEN in the service.\n\
         ListenStream=127.0.0.1:{port}\n\
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
         # Type=notify: engramd inherits the listening fd from the socket unit and signals\n\
         # readiness via sd_notify - it never binds the port itself (that would EADDRINUSE).\n\
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
        assert!(
            socket.contains("ListenStream=127.0.0.1:8088"),
            "binds loopback by default"
        );
        assert!(service.contains("Requires=engram.socket"));
        assert!(service.contains("Type=notify"));
        assert!(service.contains("NoNewPrivileges=yes"));
    }

    #[test]
    fn wake_timer_runs_due_and_exits() {
        let (svc, _timer) = wake_timer("/usr/local/bin/engramd", "*-*-* 09:00:00");
        // The wake service MUST run the due-jobs subcommand, never the bare server (which would
        // collide with the socket). Type=oneshot so systemd waits for it to finish and exit.
        assert!(svc.contains("--run-due"));
        assert!(svc.contains("Type=oneshot"));
    }

    #[test]
    fn timer_has_calendar_and_persistent() {
        let (_svc, timer) = wake_timer("/usr/local/bin/engramd", "*-*-* 09:00:00");
        assert!(timer.contains("OnCalendar=*-*-* 09:00:00"));
        assert!(timer.contains("Persistent=true"));
    }
}
