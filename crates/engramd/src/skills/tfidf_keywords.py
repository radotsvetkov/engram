#!/usr/bin/env python3
"""tfidf_keywords — Engram skill (no network). Extract keywords per document via TF-IDF.

Tokenizes each document (lowercase, \\b[a-z0-9']+\\b), drops a small English stopword
list, then scores terms by TF (count / doc length) * IDF (log(N/(1+df)) + 1). Returns
the top terms per document plus the corpus vocabulary size.

Request (stdin): {"documents": ["first doc text", "second doc text"], "top_n": 10}
Output (stdout): {document_count, corpus_vocabulary_size, top_n, documents:[{index, top_terms:[{term, tfidf}]}]}
"""
import json, sys, re, math
from collections import Counter

STOPWORDS = {
    "the", "a", "an", "and", "or", "but", "if", "then", "else", "when", "at", "by",
    "for", "with", "about", "against", "between", "into", "through", "during", "before",
    "after", "above", "below", "to", "from", "up", "down", "in", "out", "on", "off",
    "over", "under", "again", "further", "of", "is", "are", "was", "were", "be", "been",
    "being", "have", "has", "had", "do", "does", "did", "this", "that", "these", "those",
    "it", "its", "as", "i", "you", "he", "she", "we", "they", "them", "not", "no", "so",
}

_TOKEN_RE = re.compile(r"[a-z0-9']+")


def _tokens(doc):
    out = []
    for t in _TOKEN_RE.findall(doc.lower()):
        if t in STOPWORDS:
            continue
        if not t.strip("'"):
            continue
        out.append(t)
    return out


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"documents": ["first doc", "second doc"], "top_n": 10},
        })); return 0

    documents = q.get("documents")
    if not isinstance(documents, list) or not documents:
        print(json.dumps({
            "error": "missing required field 'documents' (non-empty list of strings)",
            "example": {"documents": ["first doc", "second doc"], "top_n": 10},
        })); return 0

    try:
        for d in documents:
            if not isinstance(d, str):
                print(json.dumps({"error": "'documents' must be a list of strings"})); return 0
        top_n = q.get("top_n", 10)
        if not isinstance(top_n, int) or isinstance(top_n, bool) or top_n < 1:
            print(json.dumps({"error": "'top_n' must be a positive integer"})); return 0

        n_docs = len(documents)
        doc_tokens = [_tokens(d) for d in documents]
        df = Counter()
        for toks in doc_tokens:
            for term in set(toks):
                df[term] += 1

        out_docs = []
        for i, toks in enumerate(doc_tokens):
            if not toks:
                out_docs.append({"index": i, "top_terms": []})
                continue
            tf_counts = Counter(toks)
            total = len(toks)
            scores = {}
            for term, count in tf_counts.items():
                tf = count / total
                idf = math.log(n_docs / (1 + df[term])) + 1
                scores[term] = tf * idf
            ranked = sorted(scores.items(), key=lambda kv: (-kv[1], kv[0]))[:top_n]
            out_docs.append({
                "index": i,
                "top_terms": [{"term": t, "tfidf": round(s, 6)} for t, s in ranked],
            })

        result = {
            "document_count": n_docs,
            "corpus_vocabulary_size": len(df),
            "top_n": top_n,
            "documents": out_docs,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "tfidf_keywords failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
