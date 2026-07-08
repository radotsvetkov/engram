#!/usr/bin/env python3
"""os_automation_script_gen — Engram skill (no network). Generate an
idiomatic OS-automation script/snippet for a given (os_target, action) pair
across macOS, Windows, and Linux (open_app, screenshot, notification,
set_volume, run_shell_command). Exactly like browser_automation_script_gen,
this skill only WRITES the script text — it does NOT itself launch an app,
take a screenshot, change system volume, send a notification, or run any
command. The generated snippet must be run separately (by a human or
another tool) to actually perform the OS action.

Request (stdin): {
  "os_target": "macos",
  "action": "notification",
  "params": {"title": "Build done", "message": "All tests passed"}
}
Output (stdout): {os_target, action, script, requires_note}
"""
import json
import os
import shlex
import sys

_OS_TARGETS = ["macos", "windows", "linux"]
_ACTIONS = ["open_app", "screenshot", "notification", "set_volume", "run_shell_command"]

_EXAMPLE = {
    "os_target": "macos",
    "action": "notification",
    "params": {"title": "Build done", "message": "All tests passed"},
}


def _applescript_string(s):
    return s.replace("\\", "\\\\").replace('"', '\\"')


def _osascript_cmd(src):
    return "osascript -e %s" % shlex.quote(src)


def _ps_string(s):
    return s.replace("`", "``").replace('"', '`"')


def _looks_like_file_or_url(s):
    if s.startswith(("http://", "https://", "/", "./", "~")):
        return True
    base = os.path.basename(s)
    return "." in base and " " not in s


def _gen_macos(action, params):
    if action == "open_app":
        app_name = params["app_name"]
        src = 'tell application "%s" to activate' % _applescript_string(app_name)
        return _osascript_cmd(src), None
    if action == "screenshot":
        output_path = params.get("output_path") or "screenshot.png"
        return "screencapture -x %s" % shlex.quote(output_path), None
    if action == "notification":
        message = params["message"]
        title = params.get("title") or "Notification"
        src = 'display notification "%s" with title "%s"' % (
            _applescript_string(message), _applescript_string(title))
        return _osascript_cmd(src), None
    if action == "set_volume":
        level = int(params["level"])
        src = "set volume output volume %d" % level
        return _osascript_cmd(src), None
    if action == "run_shell_command":
        return params["command"], None
    raise AssertionError(action)


def _gen_windows(action, params):
    if action == "open_app":
        app_name = params["app_name"]
        return 'Start-Process "%s"' % _ps_string(app_name), None
    if action == "screenshot":
        output_path = params.get("output_path") or "screenshot.png"
        script = (
            "Add-Type -AssemblyName System.Windows.Forms\n"
            "Add-Type -AssemblyName System.Drawing\n"
            "$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds\n"
            "$bitmap = New-Object System.Drawing.Bitmap $screen.Width, $screen.Height\n"
            "$graphics = [System.Drawing.Graphics]::FromImage($bitmap)\n"
            "$graphics.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)\n"
            '$bitmap.Save("%s", [System.Drawing.Imaging.ImageFormat]::Png)' % _ps_string(output_path)
        )
        return script, None
    if action == "notification":
        message = params["message"]
        title = params.get("title") or "Notification"
        script = (
            "# Requires: Install-Module -Name BurntToast -Scope CurrentUser\n"
            'New-BurntToastNotification -Text "%s", "%s"\n'
            "\n"
            "# Fallback without BurntToast (no extra module needed):\n"
            "# Add-Type -AssemblyName System.Windows.Forms\n"
            '# [System.Windows.Forms.MessageBox]::Show("%s", "%s")'
            % (_ps_string(title), _ps_string(message), _ps_string(message), _ps_string(title))
        )
        note = ("Requires the BurntToast PowerShell module "
                "(Install-Module -Name BurntToast -Scope CurrentUser); "
                "a MessageBox fallback needing no extra module is included as a comment.")
        return script, note
    if action == "set_volume":
        level = int(params["level"])
        nircmd_value = round(level / 100 * 65535)
        script = (
            "# Requires: nircmd.exe (https://www.nirsoft.net/utils/nircmd.html) on PATH\n"
            "nircmd.exe setsysvolume %d" % nircmd_value
        )
        note = ("Requires nircmd.exe, a common third-party utility, since Windows has "
                "no first-party CLI volume control.")
        return script, note
    if action == "run_shell_command":
        return params["command"], None
    raise AssertionError(action)


def _gen_linux(action, params):
    if action == "open_app":
        app_name = params["app_name"]
        if _looks_like_file_or_url(app_name):
            return "xdg-open %s" % shlex.quote(app_name), None
        return "%s &" % shlex.quote(app_name), None
    if action == "screenshot":
        output_path = params.get("output_path") or "screenshot.png"
        script = (
            "# requires: scrot (alternatives: gnome-screenshot, or `import` from ImageMagick)\n"
            "scrot %s" % shlex.quote(output_path)
        )
        note = "Requires the scrot package (alternatives: gnome-screenshot, ImageMagick's import)."
        return script, note
    if action == "notification":
        message = params["message"]
        title = params.get("title") or "Notification"
        script = "notify-send %s %s" % (shlex.quote(title), shlex.quote(message))
        note = "Requires libnotify-bin (provides notify-send)."
        return script, note
    if action == "set_volume":
        level = int(params["level"])
        script = "amixer set Master %d%%" % level
        note = "Requires ALSA's amixer (commonly preinstalled on most Linux distributions)."
        return script, note
    if action == "run_shell_command":
        return params["command"], None
    raise AssertionError(action)


_GENERATORS = {"macos": _gen_macos, "windows": _gen_windows, "linux": _gen_linux}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    os_target = q.get("os_target")
    if os_target not in _OS_TARGETS:
        print(json.dumps({
            "error": "'os_target' must be one of %s, got %r" % (_OS_TARGETS, os_target),
            "supported_os_targets": _OS_TARGETS,
            "example": _EXAMPLE,
        }))
        return 0

    action = q.get("action")
    if action not in _ACTIONS:
        print(json.dumps({
            "error": "'action' must be one of %s, got %r" % (_ACTIONS, action),
            "supported_actions": _ACTIONS,
            "example": _EXAMPLE,
        }))
        return 0

    params = q.get("params")
    if params is None:
        params = {}
    if not isinstance(params, dict):
        print(json.dumps({"error": "'params' must be a JSON object if provided", "example": _EXAMPLE}))
        return 0

    if action == "open_app":
        if not isinstance(params.get("app_name"), str) or not params["app_name"].strip():
            print(json.dumps({
                "error": "action 'open_app' requires non-empty 'params.app_name'",
                "example": {"os_target": os_target, "action": "open_app", "params": {"app_name": "Safari"}},
            }))
            return 0
    elif action == "notification":
        if not isinstance(params.get("message"), str) or not params["message"].strip():
            print(json.dumps({
                "error": "action 'notification' requires non-empty 'params.message'",
                "example": {
                    "os_target": os_target, "action": "notification",
                    "params": {"title": "Hello", "message": "World"},
                },
            }))
            return 0
    elif action == "set_volume":
        level = params.get("level")
        if isinstance(level, bool) or not isinstance(level, (int, float)) or not (0 <= level <= 100):
            print(json.dumps({
                "error": "action 'set_volume' requires 'params.level' as a number 0-100",
                "example": {"os_target": os_target, "action": "set_volume", "params": {"level": 50}},
            }))
            return 0
    elif action == "run_shell_command":
        if not isinstance(params.get("command"), str) or not params["command"].strip():
            print(json.dumps({
                "error": "action 'run_shell_command' requires non-empty 'params.command'",
                "example": {
                    "os_target": os_target, "action": "run_shell_command",
                    "params": {"command": "echo hello"},
                },
            }))
            return 0
    # screenshot: no required params (output_path is optional)

    try:
        script, requires_note = _GENERATORS[os_target](action, params)
    except Exception as e:
        print(json.dumps({"error": "internal error generating script: %s" % e}))
        return 1

    print(json.dumps({
        "os_target": os_target,
        "action": action,
        "script": script,
        "requires_note": requires_note,
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
