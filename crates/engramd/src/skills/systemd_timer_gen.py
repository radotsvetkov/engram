#!/usr/bin/env python3
"""systemd_timer_gen — Engram skill (no network). Generate a systemd
.service + paired .timer unit pair — the modern alternative to a plain
crontab entry — for running a command on a schedule.

The `schedule` field is a systemd OnCalendar= expression (e.g. "daily",
"*-*-* 03:00:00", "Mon,Wed,Fri 09:00"); it's embedded as-is, this skill does
not validate the full systemd calendar-event grammar.

Request (stdin): {"unit_name": "backup-db", "command": "/usr/local/bin/backup.sh", "schedule": "daily", "description": "Nightly DB backup"}
Output (stdout): {service_filename, service_content, timer_filename, timer_content, enable_command}
"""
import json
import re
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"unit_name": "backup-db", "command": "/usr/local/bin/backup.sh", "schedule": "daily"},
        }))
        return 0

    unit_name = q.get("unit_name")
    if not isinstance(unit_name, str) or not unit_name.strip():
        print(json.dumps({
            "error": "missing required field 'unit_name' (non-empty string)",
            "example": {"unit_name": "backup-db", "command": "/usr/local/bin/backup.sh", "schedule": "daily"},
        }))
        return 0
    unit_name = unit_name.strip()
    if not re.match(r"^[A-Za-z0-9_.-]+$", unit_name):
        print(json.dumps({
            "error": "'unit_name' must contain only letters, numbers, '-', '_', '.' (systemd unit name), got %r" % unit_name,
        }))
        return 0

    command = q.get("command")
    if not isinstance(command, str) or not command.strip():
        print(json.dumps({
            "error": "missing required field 'command' (non-empty string — the ExecStart command)",
            "example": {"unit_name": "backup-db", "command": "/usr/local/bin/backup.sh", "schedule": "daily"},
        }))
        return 0
    command = command.strip()

    schedule = q.get("schedule")
    if not isinstance(schedule, str) or not schedule.strip():
        print(json.dumps({
            "error": "missing required field 'schedule' (a systemd OnCalendar= expression, e.g. 'daily')",
            "example": {"unit_name": "backup-db", "command": "/usr/local/bin/backup.sh", "schedule": "daily"},
        }))
        return 0
    schedule = schedule.strip()

    description = q.get("description")
    if description is not None and not isinstance(description, str):
        print(json.dumps({"error": "'description' must be a string if provided"}))
        return 0
    description = description.strip() if isinstance(description, str) and description.strip() else (
        "Run %s" % unit_name)

    try:
        service_lines = [
            "[Unit]",
            "Description=%s" % description,
            "",
            "[Service]",
            "Type=oneshot",
            "ExecStart=%s" % command,
            "",
        ]
        service_content = "\n".join(service_lines)

        timer_lines = [
            "[Unit]",
            "Description=Timer for %s" % description,
            "",
            "[Timer]",
            "OnCalendar=%s" % schedule,
            "Persistent=true",
            "",
            "[Install]",
            "WantedBy=timers.target",
            "",
        ]
        timer_content = "\n".join(timer_lines)

        result = {
            "service_filename": "%s.service" % unit_name,
            "service_content": service_content,
            "timer_filename": "%s.timer" % unit_name,
            "timer_content": timer_content,
            "enable_command": "systemctl enable --now %s.timer" % unit_name,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "systemd_timer_gen failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
