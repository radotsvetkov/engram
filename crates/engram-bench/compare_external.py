#!/usr/bin/env python3
"""Head-to-head recall-quality comparison: mem0 (infer=False) vs LangChain (FAISS).

Uses the EXACT SAME 17-query/8-distractor corpus and scoring methodology as
crates/engram-bench/src/main.rs (`cases()`/`distractors()`/`lexical_overlap()`/`score()`), so the
numbers are directly comparable to Engram's own three-arm (keyword-only/semantic-only/hybrid)
benchmark output - see BENCHMARKS.md §3 for the full write-up and result table (including Engram's
own numbers side by side).

Each system uses its own natural, out-of-the-box local embedding path - nothing tuned or
cherry-picked:
- mem0: `Memory.add(..., infer=False)` - a real, documented mode (verified in mem0's own source:
  this path only calls the embedder, never an LLM) - mem0's own default HuggingFace embedder
  (multi-qa-MiniLM-L6-cos-v1), local on-disk Qdrant vector store.
- LangChain: HuggingFaceEmbeddings (all-MiniLM-L6-v2) + FAISS, pure similarity search, no LLM
  anywhere in the path.

Setup (Python 3.11+ required - mem0's Qdrant backend uses `X | None` union syntax that needs 3.10+):
    python3.11 -m venv /tmp/cmp_venv
    /tmp/cmp_venv/bin/pip install mem0ai langchain langchain-community sentence-transformers faiss-cpu
    OPENAI_API_KEY=sk-dummy-not-used /tmp/cmp_venv/bin/python3 compare_external.py

The dummy OPENAI_API_KEY works around a real, minor wart in mem0's own design: `Memory()` eagerly
constructs an LLM client at init even when you'll only ever call `add(infer=False)`, and the openai
SDK's client constructor fails if no key is present anywhere - but since infer=False never actually
invokes `.chat.completions`, no network call to OpenAI ever happens.
"""
import os
import sys
import tempfile

os.environ.setdefault("OPENAI_API_KEY", "sk-dummy-not-used")
os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")

CASES = [
    ("user preferences for dark themes in the editor", "preferred theming"),
    ("the agent consolidates memories overnight", "memory consolidation while sleeping"),
    ("scheduling recurring reminders every morning", "recurrent schedules"),
    ("skills are sandboxed programs that improve with use", "sandboxing improvable programs"),
    ("Engram runs on a cheap virtual private server", "running cheaply on a VPS"),
    ("the ledger is signed and tamper evident", "tamper-evident signing"),
    ("Radoslav prefers minimal dependencies", "minimal dependency preference"),
    ("embeddings turn text into vectors for semantic search", "vector embedding for meaning"),
    ("the core sleeps to zero memory when idle", "idle sleeping to zero"),
    ("the capital of France is Paris", "what is the capital of France"),
    ("WebAssembly modules run in a fuel-bounded sandbox", "fuel bounded wasm sandboxing"),
    ("recall fuses keyword and semantic ranking", "fusing semantic and keyword ranks"),
    ("she bought a new automobile last week", "purchasing a car recently"),
    ("the physician prescribed rest and fluids", "advice from a doctor"),
    ("the film received glowing reviews", "the movie got great write-ups"),
    ("he is fluent in several tongues", "speaks many languages"),
    ("the firm hired a dozen new staff", "the company recruited employees"),
]
DISTRACTORS = [
    "the weather in Berlin is mild in spring",
    "coffee is brewed from roasted beans",
    "the train departs from platform nine",
    "photosynthesis converts light into energy",
    "the meeting was rescheduled to Thursday",
    "mountains are formed by tectonic activity",
    "the recipe calls for two cups of flour",
    "satellites orbit the planet every ninety minutes",
]
STOP = {"the", "a", "an", "is", "are", "for", "in", "on", "of", "to", "and", "with", "that",
        "into", "while", "every", "what", "at", "it", "as", "by"}


def tokens(s):
    import re
    return [t for t in re.split(r"[^a-z0-9]+", s.lower()) if len(t) >= 2 and t not in STOP]


def lexical_overlap(query, fact):
    f = set(tokens(fact))
    return any(t in f for t in tokens(query))


def score(results_fn):
    """results_fn(query) -> ordered list of fact strings (top-k). Mirrors engram-bench's score()."""
    hits = 0
    mrr = 0.0
    hard_total = 0
    hard_hits = 0
    n = len(CASES)
    for fact, query in CASES:
        ranked = results_fn(query)
        hard = not lexical_overlap(query, fact)
        if hard:
            hard_total += 1
        pos = None
        for i, r in enumerate(ranked):
            if r.strip() == fact.strip():
                pos = i
                break
        if pos is not None:
            hits += 1
            mrr += 1.0 / (pos + 1)
            if hard:
                hard_hits += 1
    return {
        "recall_pct": 100.0 * hits / n,
        "hits": hits,
        "n": n,
        "mrr": mrr / n,
        "hard_pct": (100.0 * hard_hits / hard_total) if hard_total else 0.0,
        "hard_hits": hard_hits,
        "hard_total": hard_total,
    }


def row(label, s):
    print(f"| {label} | {s['recall_pct']:.0f}% ({s['hits']}/{s['n']}) | {s['mrr']:.3f} | "
          f"{s['hard_pct']:.0f}% ({s['hard_hits']}/{s['hard_total']}) |")


def bench_mem0():
    from mem0 import Memory
    tmpdir = tempfile.mkdtemp(prefix="mem0cmp_")
    config = {
        "embedder": {"provider": "huggingface",
                     "config": {"model": "multi-qa-MiniLM-L6-cos-v1"}},
        "vector_store": {"provider": "qdrant",
                          "config": {"path": os.path.join(tmpdir, "qdrant"), "on_disk": True,
                                     "collection_name": "cmp", "embedding_model_dims": 384}},
    }
    m = Memory.from_config(config)
    user_id = "bench-user"
    for fact, _ in CASES:
        m.add(fact, user_id=user_id, infer=False)
    for d in DISTRACTORS:
        m.add(d, user_id=user_id, infer=False)

    def results_fn(query):
        out = m.search(query, filters={"user_id": user_id}, limit=10)
        items = out["results"] if isinstance(out, dict) and "results" in out else out
        return [it["memory"] for it in items]

    return score(results_fn)


def bench_langchain():
    from langchain_community.embeddings import HuggingFaceEmbeddings
    from langchain_community.vectorstores import FAISS
    emb = HuggingFaceEmbeddings(model_name="all-MiniLM-L6-v2")
    texts = [fact for fact, _ in CASES] + DISTRACTORS
    vs = FAISS.from_texts(texts, emb)

    def results_fn(query):
        docs = vs.similarity_search(query, k=10)
        return [d.page_content for d in docs]

    return score(results_fn)


def main():
    print("# External comparison: mem0 (infer=False) vs LangChain (FAISS)\n")
    print(f"Corpus: {len(CASES) + len(DISTRACTORS)} facts. Queries: {len(CASES)}. Recall@10.\n")
    print("| System | Recall@10 | MRR | Recall@10 on zero-overlap paraphrases |")
    print("|---|---|---|---|")
    try:
        row("mem0 (infer=False, HF multi-qa-MiniLM-L6-cos-v1, local Qdrant)", bench_mem0())
    except Exception as e:
        print(f"mem0 FAILED: {e}", file=sys.stderr)
        raise
    try:
        row("LangChain (HF all-MiniLM-L6-v2, FAISS)", bench_langchain())
    except Exception as e:
        print(f"LangChain FAILED: {e}", file=sys.stderr)
        raise


if __name__ == "__main__":
    main()
