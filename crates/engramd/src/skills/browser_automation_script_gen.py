#!/usr/bin/env python3
"""browser_automation_script_gen — Engram skill (no network). Generate
reusable browser-automation/test boilerplate code for Playwright (Python or
Node) or Selenium (Python). This skill only WRITES code — it does not itself
launch a browser or perform any live action; the generated script must be
run separately to actually drive a browser.

Request (stdin): {
  "framework": "playwright_python",
  "url": "https://example.com",
  "actions": [
    {"type": "fill", "selector": "#email", "value": "a@b.com"},
    {"type": "click", "selector": "#submit"},
    {"type": "wait_for", "selector": ".success"},
    {"type": "screenshot", "value": "result.png"}
  ]
}
Output (stdout): {filename, code}
"""
import json
import sys

_SUPPORTED_FRAMEWORKS = ["playwright_python", "playwright_node", "selenium_python"]
_SUPPORTED_ACTIONS = ["click", "fill", "wait_for", "screenshot"]


def _py_str(s):
    return json.dumps(s)


def _gen_playwright_python(url, actions):
    lines = []
    lines.append("from playwright.sync_api import sync_playwright")
    lines.append("")
    lines.append("")
    lines.append("def run():")
    lines.append("    with sync_playwright() as p:")
    lines.append("        browser = p.chromium.launch(headless=True)")
    lines.append("        page = browser.new_page()")
    lines.append("        page.goto(%s)" % _py_str(url))
    for a in actions:
        t = a["type"]
        if t == "click":
            lines.append("        page.click(%s)" % _py_str(a["selector"]))
        elif t == "fill":
            lines.append("        page.fill(%s, %s)" % (_py_str(a["selector"]), _py_str(a.get("value", ""))))
        elif t == "wait_for":
            lines.append("        page.wait_for_selector(%s)" % _py_str(a["selector"]))
        elif t == "screenshot":
            path = a.get("value") or "screenshot.png"
            lines.append("        page.screenshot(path=%s)" % _py_str(path))
    lines.append("        browser.close()")
    lines.append("")
    lines.append("")
    lines.append("if __name__ == \"__main__\":")
    lines.append("    run()")
    lines.append("")
    return "\n".join(lines)


def _gen_playwright_node(url, actions):
    lines = []
    lines.append("const { chromium } = require('playwright');")
    lines.append("")
    lines.append("(async () => {")
    lines.append("  const browser = await chromium.launch({ headless: true });")
    lines.append("  const page = await browser.newPage();")
    lines.append("  await page.goto(%s);" % json.dumps(url))
    for a in actions:
        t = a["type"]
        if t == "click":
            lines.append("  await page.click(%s);" % json.dumps(a["selector"]))
        elif t == "fill":
            lines.append("  await page.fill(%s, %s);" % (json.dumps(a["selector"]), json.dumps(a.get("value", ""))))
        elif t == "wait_for":
            lines.append("  await page.waitForSelector(%s);" % json.dumps(a["selector"]))
        elif t == "screenshot":
            path = a.get("value") or "screenshot.png"
            lines.append("  await page.screenshot({ path: %s });" % json.dumps(path))
    lines.append("  await browser.close();")
    lines.append("})();")
    lines.append("")
    return "\n".join(lines)


def _gen_selenium_python(url, actions):
    lines = []
    lines.append("from selenium import webdriver")
    lines.append("from selenium.webdriver.common.by import By")
    lines.append("from selenium.webdriver.support import expected_conditions as EC")
    lines.append("from selenium.webdriver.support.ui import WebDriverWait")
    lines.append("")
    lines.append("")
    lines.append("def run():")
    lines.append("    driver = webdriver.Chrome()")
    lines.append("    try:")
    lines.append("        driver.get(%s)" % _py_str(url))
    for a in actions:
        t = a["type"]
        if t == "click":
            lines.append("        driver.find_element(By.CSS_SELECTOR, %s).click()" % _py_str(a["selector"]))
        elif t == "fill":
            lines.append("        driver.find_element(By.CSS_SELECTOR, %s).send_keys(%s)" % (
                _py_str(a["selector"]), _py_str(a.get("value", ""))))
        elif t == "wait_for":
            lines.append("        WebDriverWait(driver, 10).until(")
            lines.append("            EC.presence_of_element_located((By.CSS_SELECTOR, %s))" % _py_str(a["selector"]))
            lines.append("        )")
        elif t == "screenshot":
            path = a.get("value") or "screenshot.png"
            lines.append("        driver.save_screenshot(%s)" % _py_str(path))
    lines.append("    finally:")
    lines.append("        driver.quit()")
    lines.append("")
    lines.append("")
    lines.append("if __name__ == \"__main__\":")
    lines.append("    run()")
    lines.append("")
    return "\n".join(lines)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {
                "framework": "playwright_python",
                "url": "https://example.com",
                "actions": [{"type": "click", "selector": "#submit"}],
            },
        }))
        return 0

    framework = q.get("framework")
    if not isinstance(framework, str) or framework not in _SUPPORTED_FRAMEWORKS:
        print(json.dumps({
            "error": "'framework' must be one of %s, got %r" % (_SUPPORTED_FRAMEWORKS, framework),
            "supported_frameworks": _SUPPORTED_FRAMEWORKS,
        }))
        return 0

    url = q.get("url")
    if not isinstance(url, str) or not url.strip():
        print(json.dumps({
            "error": "missing required field 'url' (non-empty string)",
            "example": {"framework": framework, "url": "https://example.com", "actions": []},
        }))
        return 0
    url = url.strip()

    actions = q.get("actions")
    if actions is None:
        actions = []
    if not isinstance(actions, list):
        print(json.dumps({"error": "'actions' must be a list if provided"}))
        return 0

    validated = []
    for i, a in enumerate(actions):
        if not isinstance(a, dict):
            print(json.dumps({"error": "action at index %d must be a JSON object" % i}))
            return 0
        t = a.get("type")
        if t not in _SUPPORTED_ACTIONS:
            print(json.dumps({
                "error": "action at index %d has unsupported 'type' %r" % (i, t),
                "supported_action_types": _SUPPORTED_ACTIONS,
            }))
            return 0
        if t in ("click", "wait_for") and (not isinstance(a.get("selector"), str) or not a.get("selector").strip()):
            print(json.dumps({"error": "action at index %d (type=%r) requires a non-empty 'selector'" % (i, t)}))
            return 0
        if t == "fill" and (not isinstance(a.get("selector"), str) or not a.get("selector").strip()):
            print(json.dumps({"error": "action at index %d (type='fill') requires a non-empty 'selector'" % i}))
            return 0
        validated.append(a)

    try:
        if framework == "playwright_python":
            code = _gen_playwright_python(url, validated)
            filename = "automation_script.py"
        elif framework == "playwright_node":
            code = _gen_playwright_node(url, validated)
            filename = "automation_script.js"
        else:
            code = _gen_selenium_python(url, validated)
            filename = "automation_script.py"

        print(json.dumps({"filename": filename, "code": code}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "browser_automation_script_gen failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
