#!/usr/bin/env python3
"""pptx_minimal_gen — Engram skill (no network). Generate a minimal, real .pptx file.

Hand-builds a valid Office Open XML PowerPoint package (a zip of XML parts) —
no python-pptx or any third-party library, since the sandbox is stdlib-only.
Each slide gets a title and a bulleted content list using the standard
"Title and Content" layout. Every embedded XML part is verified to be
well-formed (xml.etree.ElementTree) and the resulting zip's integrity is
checked (zipfile.testzip()) before returning it. This has NOT been verified
against every PowerPoint/LibreOffice version (no office suite was available
to render-test it here) — it follows the documented minimal OOXML structure,
but treat it as a best-effort minimal deck, not a fully-featured export.

Request (stdin): {"title": "Deck Title", "slides": [{"title": "Slide 1",
                  "bullets": ["point one", "point two"]}, ...]}
Output (stdout): {filename: "deck.pptx", data_base64: "...", slide_count}
"""
import base64
import io
import json
import sys
import xml.etree.ElementTree as ET
import zipfile
from xml.sax.saxutils import escape as xml_escape

NSMAP = {
    "p": "http://schemas.openxmlformats.org/presentationml/2006/main",
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "ct": "http://schemas.openxmlformats.org/package/2006/content-types",
    "rel": "http://schemas.openxmlformats.org/package/2006/relationships",
    "cp": "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
    "ep": "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties",
}

XML_DECL = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'


def content_types_xml(n_slides):
    overrides = "".join(
        '<Override PartName="/ppt/slides/slide%d.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>' % i
        for i in range(1, n_slides + 1)
    )
    return XML_DECL + (
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/ppt/presentation.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>'
        '<Override PartName="/ppt/slideMasters/slideMaster1.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>'
        '<Override PartName="/ppt/slideLayouts/slideLayout1.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>'
        '<Override PartName="/ppt/theme/theme1.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>'
        '<Override PartName="/docProps/core.xml" '
        'ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>'
        '<Override PartName="/docProps/app.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>'
        + overrides +
        "</Types>"
    )


def root_rels_xml():
    return XML_DECL + (
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" '
        'Target="ppt/presentation.xml"/>'
        '<Relationship Id="rId2" '
        'Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" '
        'Target="docProps/core.xml"/>'
        '<Relationship Id="rId3" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" '
        'Target="docProps/app.xml"/>'
        "</Relationships>"
    )


def core_props_xml(title):
    return XML_DECL + (
        '<cp:coreProperties '
        'xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
        'xmlns:dc="http://purl.org/dc/elements/1.1/" '
        'xmlns:dcterms="http://purl.org/dc/terms/" '
        'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
        "<dc:title>%s</dc:title>"
        "<dc:creator>Engram</dc:creator>"
        "<cp:lastModifiedBy>Engram</cp:lastModifiedBy>"
        "</cp:coreProperties>"
    ) % xml_escape(title)


def app_props_xml(n_slides):
    return XML_DECL + (
        '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" '
        'xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">'
        "<Application>Engram</Application>"
        "<Slides>%d</Slides>"
        "</Properties>"
    ) % n_slides


def presentation_xml(n_slides):
    sld_ids = "".join(
        '<p:sldId id="%d" r:id="rIdSld%d"/>' % (256 + i, i + 1) for i in range(n_slides)
    )
    return XML_DECL + (
        '<p:presentation '
        'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
        'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">'
        '<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdMaster1"/></p:sldMasterIdLst>'
        "<p:sldIdLst>%s</p:sldIdLst>"
        '<p:sldSz cx="9144000" cy="6858000" type="screen4x3"/>'
        '<p:notesSz cx="6858000" cy="9144000"/>'
        "</p:presentation>"
    ) % sld_ids


def presentation_rels_xml(n_slides):
    slide_rels = "".join(
        '<Relationship Id="rIdSld%d" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" '
        'Target="slides/slide%d.xml"/>' % (i + 1, i + 1)
        for i in range(n_slides)
    )
    return XML_DECL + (
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rIdMaster1" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" '
        'Target="slideMasters/slideMaster1.xml"/>'
        + slide_rels +
        "</Relationships>"
    )


def slide_master_xml():
    return XML_DECL + (
        '<p:sldMaster '
        'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
        'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">'
        "<p:cSld><p:bg><p:bgRef idx=\"1001\"><a:schemeClr val=\"bg1\"/></p:bgRef></p:bg>"
        "<p:spTree>"
        '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>'
        "<p:grpSpPr/>"
        "</p:spTree></p:cSld>"
        '<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" '
        'accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" '
        'hlink="hlink" folHlink="folHlink"/>'
        '<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst>'
        "</p:sldMaster>"
    )


def slide_master_rels_xml():
    return XML_DECL + (
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" '
        'Target="../slideLayouts/slideLayout1.xml"/>'
        '<Relationship Id="rId2" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" '
        'Target="../theme/theme1.xml"/>'
        "</Relationships>"
    )


def slide_layout_xml():
    return XML_DECL + (
        '<p:sldLayout '
        'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
        'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" '
        'type="title" preserve="1">'
        '<p:cSld name="Title and Content"><p:spTree>'
        '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>'
        "<p:grpSpPr/>"
        "</p:spTree></p:cSld>"
        "<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>"
        "</p:sldLayout>"
    )


def slide_layout_rels_xml():
    return XML_DECL + (
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" '
        'Target="../slideMasters/slideMaster1.xml"/>'
        "</Relationships>"
    )


def theme_xml():
    return XML_DECL + (
        '<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Engram Theme">'
        "<a:themeElements>"
        '<a:clrScheme name="Engram">'
        '<a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>'
        '<a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>'
        '<a:dk2><a:srgbClr val="1F1F1F"/></a:dk2>'
        '<a:lt2><a:srgbClr val="EEEEEE"/></a:lt2>'
        '<a:accent1><a:srgbClr val="4472C4"/></a:accent1>'
        '<a:accent2><a:srgbClr val="ED7D31"/></a:accent2>'
        '<a:accent3><a:srgbClr val="A5A5A5"/></a:accent3>'
        '<a:accent4><a:srgbClr val="FFC000"/></a:accent4>'
        '<a:accent5><a:srgbClr val="5B9BD5"/></a:accent5>'
        '<a:accent6><a:srgbClr val="70AD47"/></a:accent6>'
        '<a:hlink><a:srgbClr val="0563C1"/></a:hlink>'
        '<a:folHlink><a:srgbClr val="954F72"/></a:folHlink>'
        "</a:clrScheme>"
        '<a:fontScheme name="Engram">'
        '<a:majorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>'
        '<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont>'
        "</a:fontScheme>"
        '<a:fmtScheme name="Engram">'
        '<a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
        '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
        '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>'
        '<a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>'
        '<a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>'
        '<a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>'
        "<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle>"
        "<a:effectStyle><a:effectLst/></a:effectStyle>"
        "<a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>"
        '<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
        '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
        '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst>'
        "</a:fmtScheme>"
        "</a:themeElements>"
        "</a:theme>"
    )


def slide_xml(title, bullets):
    bullet_paras = "".join(
        '<a:p><a:pPr marL="285750" indent="-285750"><a:buChar char="•"/></a:pPr>'
        '<a:r><a:rPr lang="en-US" sz="1800"/><a:t>%s</a:t></a:r></a:p>' % xml_escape(b)
        for b in bullets
    ) or '<a:p><a:r><a:rPr lang="en-US" sz="1800"/><a:t></a:t></a:r></a:p>'
    return XML_DECL + (
        '<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
        'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">'
        "<p:cSld><p:spTree>"
        '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>'
        "<p:grpSpPr/>"
        "<p:sp>"
        '<p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
        '<p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>'
        '<p:spPr><a:xfrm><a:off x="457200" y="274638"/><a:ext cx="8229600" cy="1143000"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
        '<p:txBody><a:bodyPr/><a:lstStyle/>'
        '<a:p><a:r><a:rPr lang="en-US" sz="3200" b="1"/><a:t>%s</a:t></a:r></a:p>'
        "</p:txBody>"
        "</p:sp>"
        "<p:sp>"
        '<p:nvSpPr><p:cNvPr id="3" name="Content"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
        '<p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr>'
        '<p:spPr><a:xfrm><a:off x="457200" y="1600200"/><a:ext cx="8229600" cy="4525963"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
        '<p:txBody><a:bodyPr/><a:lstStyle/>%s</p:txBody>'
        "</p:sp>"
        "</p:spTree></p:cSld>"
        "<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>"
        "</p:sld>"
    ) % (xml_escape(title), bullet_paras)


def slide_rels_xml():
    return XML_DECL + (
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" '
        'Target="../slideLayouts/slideLayout1.xml"/>'
        "</Relationships>"
    )


def build_pptx(title, slides):
    parts = {}
    n = len(slides)
    parts["[Content_Types].xml"] = content_types_xml(n)
    parts["_rels/.rels"] = root_rels_xml()
    parts["docProps/core.xml"] = core_props_xml(title)
    parts["docProps/app.xml"] = app_props_xml(n)
    parts["ppt/presentation.xml"] = presentation_xml(n)
    parts["ppt/_rels/presentation.xml.rels"] = presentation_rels_xml(n)
    parts["ppt/slideMasters/slideMaster1.xml"] = slide_master_xml()
    parts["ppt/slideMasters/_rels/slideMaster1.xml.rels"] = slide_master_rels_xml()
    parts["ppt/slideLayouts/slideLayout1.xml"] = slide_layout_xml()
    parts["ppt/slideLayouts/_rels/slideLayout1.xml.rels"] = slide_layout_rels_xml()
    parts["ppt/theme/theme1.xml"] = theme_xml()
    for i, s in enumerate(slides, start=1):
        parts["ppt/slides/slide%d.xml" % i] = slide_xml(s.get("title") or "", s.get("bullets") or [])
        parts["ppt/slides/_rels/slide%d.xml.rels" % i] = slide_rels_xml()

    # Every XML part must be well-formed before we ship it.
    for name, content in parts.items():
        if name.endswith(".xml") or name.endswith(".rels"):
            ET.fromstring(content)

    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
        for name, content in parts.items():
            zf.writestr(name, content)
    data = buf.getvalue()

    # Verify zip integrity before returning it.
    with zipfile.ZipFile(io.BytesIO(data)) as zf:
        bad = zf.testzip()
        if bad is not None:
            raise RuntimeError("generated zip has a corrupt entry: %s" % bad)
    return data


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    title = q.get("title")
    slides = q.get("slides")
    if not isinstance(title, str) or not title.strip():
        print(json.dumps({
            "error": "provide a non-empty 'title'",
            "example": {"title": "Q3 Roadmap", "slides": [{"title": "Overview", "bullets": ["Point one", "Point two"]}]},
        }))
        return 0
    if not isinstance(slides, list) or not slides:
        print(json.dumps({
            "error": "provide a non-empty 'slides' list, each with a 'title' and optional 'bullets'",
            "example": {"title": "Q3 Roadmap", "slides": [{"title": "Overview", "bullets": ["Point one", "Point two"]}]},
        }))
        return 0
    for s in slides:
        if not isinstance(s, dict) or not isinstance(s.get("title"), str):
            print(json.dumps({"error": "each slide must be an object with at least a string 'title'"}))
            return 0
        bullets = s.get("bullets", [])
        if not isinstance(bullets, list) or not all(isinstance(b, str) for b in bullets):
            print(json.dumps({"error": "each slide's 'bullets', if given, must be a list of strings"}))
            return 0

    try:
        data = build_pptx(title, slides)
    except Exception as e:
        print(json.dumps({"error": "failed to generate .pptx: %s" % e}))
        return 1

    print(json.dumps({
        "filename": "deck.pptx",
        "data_base64": base64.b64encode(data).decode("ascii"),
        "slide_count": len(slides),
        "note": "minimal Office Open XML package built without python-pptx; every XML part was checked for well-formedness and the zip's integrity was verified, but this was not render-tested against a real PowerPoint/LibreOffice install",
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
