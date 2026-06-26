#!/usr/bin/env python3
"""Fetch a model2vec static embedding model for Engram's StaticEmbedder.

A model2vec model is a distilled static embedding table (a vocab + a [vocab, dim] matrix)
that gives real synonym/paraphrase recall with no neural network at inference. Engram reads
it directly in pure Rust, so this script just downloads the two files it needs -
`tokenizer.json` and `model.safetensors` - into the target directory. No Python ML deps.

Usage:
    python3 scripts/build_embedder.py [--model minishlab/potion-base-8M] [--out DIR]

Then run the daemon against it:
    ENGRAM_EMBED=static ENGRAM_STATIC_MODEL=<DIR> engramd
(or place the files at <ENGRAM_HOME>/embedder and just set ENGRAM_EMBED=static).

Existing memories are re-embedded into the new space automatically on first open.
"""
import argparse
import os
import urllib.request


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--model", default="minishlab/potion-base-8M",
                    help="HuggingFace model2vec model id (default: a small distilled BGE)")
    ap.add_argument("--out", default=os.path.join(os.environ.get("ENGRAM_HOME", "./brain"), "embedder"),
                    help="output directory (default: $ENGRAM_HOME/embedder)")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    base = f"https://huggingface.co/{args.model}/resolve/main"
    for fname in ("tokenizer.json", "model.safetensors"):
        url = f"{base}/{fname}"
        dst = os.path.join(args.out, fname)
        print(f"downloading {url}")
        urllib.request.urlretrieve(url, dst)
        print(f"  -> {dst} ({os.path.getsize(dst):,} bytes)")

    print(f"\nDone. Run the daemon with:\n  ENGRAM_EMBED=static ENGRAM_STATIC_MODEL={args.out} engramd")


if __name__ == "__main__":
    main()
