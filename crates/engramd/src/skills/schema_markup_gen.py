#!/usr/bin/env python3
"""schema_markup_gen — Engram skill (no network). Generate schema.org JSON-LD.

Builds valid JSON-LD structured data for Article, Product, FAQPage,
LocalBusiness, or Organization from a plain `fields` dict. Missing
recommended fields are best-effort omitted and listed as warnings rather than
failing. Stdlib only.

Request (stdin): {"type": "Article", "fields": {"headline": "...", "author": "..."}}
  - type: one of "Article" | "Product" | "FAQPage" | "LocalBusiness" | "Organization"
Output (stdout): {jsonld: {...}, warnings: [...]}
"""
import json
import sys

EXAMPLES = {
    "Article": {
        "headline": "How to Bake Sourdough Bread",
        "author": "Jane Doe",
        "datePublished": "2026-01-15",
        "dateModified": "2026-01-20",
        "image": "https://example.com/bread.jpg",
        "description": "A beginner's guide to sourdough.",
    },
    "Product": {
        "name": "Wireless Mouse",
        "description": "Ergonomic wireless mouse with USB-C charging.",
        "image": "https://example.com/mouse.jpg",
        "brand": "Acme",
        "price": 29.99,
        "priceCurrency": "USD",
        "sku": "ACM-MOU-01",
    },
    "FAQPage": {
        "questions": [
            {"question": "What is schema markup?", "answer": "Structured data that helps search engines understand a page."},
        ]
    },
    "LocalBusiness": {
        "name": "Joe's Coffee Shop",
        "address": "123 Main St, Springfield",
        "telephone": "+1-555-0100",
        "priceRange": "$$",
        "url": "https://example.com",
    },
    "Organization": {
        "name": "Acme Corp",
        "url": "https://example.com",
        "logo": "https://example.com/logo.png",
        "sameAs": ["https://twitter.com/acme", "https://linkedin.com/company/acme"],
    },
}


def _build_article(fields):
    warnings = []
    for f in ("headline", "author", "datePublished"):
        if not fields.get(f):
            warnings.append("missing recommended field '%s'" % f)
    jsonld = {"@context": "https://schema.org", "@type": "Article"}
    if fields.get("headline"):
        jsonld["headline"] = fields["headline"]
    if fields.get("author"):
        jsonld["author"] = {"@type": "Person", "name": fields["author"]}
    if fields.get("datePublished"):
        jsonld["datePublished"] = fields["datePublished"]
    if fields.get("dateModified"):
        jsonld["dateModified"] = fields["dateModified"]
    if fields.get("image"):
        jsonld["image"] = fields["image"]
    if fields.get("description"):
        jsonld["description"] = fields["description"]
    return jsonld, warnings


def _build_product(fields):
    warnings = []
    if not fields.get("name"):
        warnings.append("missing required field 'name'")
    jsonld = {"@context": "https://schema.org", "@type": "Product"}
    if fields.get("name"):
        jsonld["name"] = fields["name"]
    if fields.get("description"):
        jsonld["description"] = fields["description"]
    if fields.get("image"):
        jsonld["image"] = fields["image"]
    if fields.get("brand"):
        jsonld["brand"] = {"@type": "Brand", "name": fields["brand"]}
    if fields.get("sku"):
        jsonld["sku"] = fields["sku"]
    if fields.get("price") is not None:
        jsonld["offers"] = {
            "@type": "Offer",
            "price": fields["price"],
            "priceCurrency": fields.get("priceCurrency") or "USD",
        }
    return jsonld, warnings


def _build_faqpage(fields):
    warnings = []
    questions = fields.get("questions") or []
    if not isinstance(questions, list) or not questions:
        warnings.append("missing required field 'questions' (list of {question, answer})")
        questions = []
    main_entity = []
    for item in questions:
        item = item or {}
        q_text = item.get("question")
        a_text = item.get("answer")
        if not q_text or not a_text:
            warnings.append("skipped a question entry missing 'question' or 'answer'")
            continue
        main_entity.append({
            "@type": "Question",
            "name": q_text,
            "acceptedAnswer": {"@type": "Answer", "text": a_text},
        })
    jsonld = {"@context": "https://schema.org", "@type": "FAQPage", "mainEntity": main_entity}
    return jsonld, warnings


def _build_localbusiness(fields):
    warnings = []
    if not fields.get("name"):
        warnings.append("missing required field 'name'")
    jsonld = {"@context": "https://schema.org", "@type": "LocalBusiness"}
    for f in ("name", "address", "telephone", "priceRange", "url"):
        if fields.get(f):
            jsonld[f] = fields[f]
    return jsonld, warnings


def _build_organization(fields):
    warnings = []
    if not fields.get("name"):
        warnings.append("missing required field 'name'")
    jsonld = {"@context": "https://schema.org", "@type": "Organization"}
    for f in ("name", "url", "logo"):
        if fields.get(f):
            jsonld[f] = fields[f]
    if fields.get("sameAs"):
        jsonld["sameAs"] = fields["sameAs"]
    return jsonld, warnings


BUILDERS = {
    "Article": _build_article,
    "Product": _build_product,
    "FAQPage": _build_faqpage,
    "LocalBusiness": _build_localbusiness,
    "Organization": _build_organization,
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    t = (q.get("type") or "").strip()
    fields = q.get("fields")
    if fields is None:
        fields = {}
    if not isinstance(fields, dict):
        print(json.dumps({"error": "'fields' must be an object"}))
        return 0
    if t not in BUILDERS:
        print(json.dumps({
            "error": "unknown or missing 'type' '%s' — supported types: %s" % (t, ", ".join(BUILDERS)),
            "examples": EXAMPLES,
        }))
        return 0

    try:
        jsonld, warnings = BUILDERS[t](fields)
        print(json.dumps({"jsonld": jsonld, "warnings": warnings}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "failed to build schema markup: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
