#!/usr/bin/env python3
"""xml_json_convert — Engram skill (no network). Convert between XML and JSON.

XML->JSON: parse with xml.etree.ElementTree; each element becomes
{tag: {"@attr": ..., "#text": ..., child: ...}}, with repeated children collapsed
into lists. JSON->XML: emit well-formed XML from a dict, taking attributes from
"@"-prefixed keys and element text from "#text". Direction auto-detected by which
key is present. Malformed XML returns a clean error.

Request (stdin): {"xml": "<r>..</r>"}  OR  {"json": {..}|"{..}", "root"?: "root"}
Output (stdout): {json: {...}}  OR  {xml: "..."}
"""
import json, sys
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape, quoteattr


def _elem_to_obj(elem):
    node = {}
    for k, v in elem.attrib.items():
        node["@" + k] = v
    children = list(elem)
    text = (elem.text or "").strip()
    if children:
        for child in children:
            cobj = _elem_to_obj(child)
            tag = child.tag
            if tag in node:
                if not isinstance(node[tag], list):
                    node[tag] = [node[tag]]
                node[tag].append(cobj)
            else:
                node[tag] = cobj
        if text:
            node["#text"] = text
    else:
        if node:  # has attributes but no children
            if text:
                node["#text"] = text
        else:
            # leaf with only text: represent as the bare text value
            return text
    return node


def _obj_to_xml(tag, obj, indent, level):
    pad = indent * level
    if isinstance(obj, dict):
        attrs = ""
        children = []
        text = None
        for k, v in obj.items():
            if k.startswith("@"):
                attrs += " %s=%s" % (k[1:], quoteattr(str(v)))
            elif k == "#text":
                text = v
            else:
                children.append((k, v))
        if not children and text is None:
            return "%s<%s%s/>" % (pad, tag, attrs)
        parts = ["%s<%s%s>" % (pad, tag, attrs)]
        inline = text is not None and not children
        if inline:
            return "%s<%s%s>%s</%s>" % (pad, tag, attrs, escape(str(text)), tag)
        if text is not None:
            parts.append("%s%s%s" % (pad, indent, escape(str(text))))
        for ck, cv in children:
            if isinstance(cv, list):
                for item in cv:
                    parts.append(_obj_to_xml(ck, item, indent, level + 1))
            else:
                parts.append(_obj_to_xml(ck, cv, indent, level + 1))
        parts.append("%s</%s>" % (pad, tag))
        return "\n".join(parts)
    elif isinstance(obj, list):
        return "\n".join(_obj_to_xml(tag, item, indent, level) for item in obj)
    else:
        if obj is None:
            return "%s<%s/>" % (pad, tag)
        return "%s<%s>%s</%s>" % (pad, tag, escape(str(obj)), tag)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"xml": "<root><a>1</a></root>"},
        })); return 0

    has_xml = "xml" in q and q.get("xml") is not None
    has_json = "json" in q and q.get("json") is not None
    if not has_xml and not has_json:
        print(json.dumps({
            "error": "provide 'xml' (XML->JSON) or 'json' (JSON->XML)",
            "example": {"xml": "<root><a x=\"1\">hi</a></root>"},
        })); return 0

    try:
        if has_xml:
            xml_text = q.get("xml")
            if not isinstance(xml_text, str):
                print(json.dumps({"error": "'xml' must be a string"})); return 0
            try:
                root = ET.fromstring(xml_text)
            except ET.ParseError as e:
                print(json.dumps({"error": "malformed XML: %s" % e})); return 0
            result = {root.tag: _elem_to_obj(root)}
            print(json.dumps({"json": result}, indent=2, default=str)); return 0
        else:
            raw = q.get("json")
            if isinstance(raw, str):
                try:
                    data = json.loads(raw)
                except Exception as e:
                    print(json.dumps({"error": "invalid JSON in 'json': %s" % e})); return 0
            else:
                data = raw
            # If the top-level dict has exactly one key, treat it as the root tag.
            if isinstance(data, dict) and len(data) == 1:
                tag = next(iter(data))
                xml_str = _obj_to_xml(tag, data[tag], "  ", 0)
            else:
                root_tag = q.get("root") or "root"
                if not isinstance(root_tag, str) or root_tag == "":
                    root_tag = "root"
                xml_str = _obj_to_xml(root_tag, data, "  ", 0)
            print(json.dumps({"xml": xml_str}, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "xml_json_convert failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
