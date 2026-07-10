#!/usr/bin/env python3
"""wordpress_plugin_scaffold — Engram skill (no network). Scaffold the main
plugin file for a WordPress plugin: the standard header docblock, an ABSPATH
guard, activation/deactivation hooks, and a sample add_action('init', ...).

Request (stdin): {"plugin_name": "My Cool Plugin", "description": "Does things.", "author": "Jane Doe"}
Output (stdout): {files, slug, notes, next_steps}
"""
import json
import re
import sys


def _slugify(name):
    s = name.strip().lower()
    s = re.sub(r"[^a-z0-9]+", "-", s)
    return s.strip("-")


def _to_php_prefix(slug):
    # my-cool-plugin -> my_cool_plugin (function/hook prefix)
    return re.sub(r"[^a-z0-9]+", "_", slug).strip("_")


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"plugin_name": "My Cool Plugin", "description": "Does things.", "author": "Jane Doe"},
        }))
        return 0

    raw_name = q.get("plugin_name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'plugin_name' (non-empty string)",
            "example": {"plugin_name": "My Cool Plugin", "description": "Does things.", "author": "Jane Doe"},
        }))
        return 0

    description = q.get("description")
    if not isinstance(description, str) or not description.strip():
        description = "A WordPress plugin."
    author = q.get("author")
    if not isinstance(author, str) or not author.strip():
        author = "Your Name"

    try:
        slug = _slugify(raw_name)
        if not slug:
            print(json.dumps({"error": "could not derive a slug from %r" % raw_name}))
            return 0
        prefix = _to_php_prefix(slug)

        L = []
        L.append("<?php")
        L.append("/**")
        L.append(" * Plugin Name:       %s" % raw_name.strip())
        L.append(" * Description:       %s" % description.strip())
        L.append(" * Version:           0.1.0")
        L.append(" * Requires at least: 6.0")
        L.append(" * Requires PHP:      7.4")
        L.append(" * Author:            %s" % author.strip())
        L.append(" * License:           GPL-2.0-or-later")
        L.append(" * License URI:       https://www.gnu.org/licenses/gpl-2.0.html")
        L.append(" * Text Domain:       %s" % slug)
        L.append(" */")
        L.append("")
        L.append("if ( ! defined( 'ABSPATH' ) ) {")
        L.append("    exit; // Exit if accessed directly.")
        L.append("}")
        L.append("")
        L.append("define( '%s_VERSION', '0.1.0' );" % prefix.upper())
        L.append("define( '%s_PATH', plugin_dir_path( __FILE__ ) );" % prefix.upper())
        L.append("define( '%s_URL', plugin_dir_url( __FILE__ ) );" % prefix.upper())
        L.append("")
        L.append("/**")
        L.append(" * Runs on plugin activation.")
        L.append(" */")
        L.append("function %s_activate() {" % prefix)
        L.append("    // TODO: set default options, create tables, etc.")
        L.append("    flush_rewrite_rules();")
        L.append("}")
        L.append("register_activation_hook( __FILE__, '%s_activate' );" % prefix)
        L.append("")
        L.append("/**")
        L.append(" * Runs on plugin deactivation.")
        L.append(" */")
        L.append("function %s_deactivate() {" % prefix)
        L.append("    // TODO: clean up scheduled events, transients, etc.")
        L.append("    flush_rewrite_rules();")
        L.append("}")
        L.append("register_deactivation_hook( __FILE__, '%s_deactivate' );" % prefix)
        L.append("")
        L.append("/**")
        L.append(" * Bootstrap the plugin.")
        L.append(" */")
        L.append("function %s_init() {" % prefix)
        L.append("    load_plugin_textdomain( '%s', false, dirname( plugin_basename( __FILE__ ) ) . '/languages' );" % slug)
        L.append("    // TODO: register post types, shortcodes, hooks, etc.")
        L.append("}")
        L.append("add_action( 'init', '%s_init' );" % prefix)
        L.append("")
        code = "\n".join(L)

        path = "%s/%s.php" % (slug, slug)
        result = {
            "files": {path: code},
            "slug": slug,
            "notes": [
                "Main plugin file with the standard WordPress plugin header docblock.",
                "Includes the `if ( ! defined( 'ABSPATH' ) ) exit;` direct-access guard.",
                "Activation + deactivation hooks and an add_action('init', ...) bootstrap.",
                "Function/hook prefix is `%s_` and the text domain is `%s`." % (prefix, slug),
            ],
            "next_steps": [
                "Place the folder `%s/` in wp-content/plugins/ and activate it from the Plugins screen." % slug,
                "Create a /languages directory for translation .po/.mo files (text domain `%s`)." % slug,
                "Add a readme.txt (WordPress.org header format) if you plan to publish it.",
            ],
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "wordpress_plugin_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
