# /// script
# requires-python = ">=3.11"
# dependencies = ["requests>=2.31"]
# ///
"""One-time enrichment generator for tuna — bakes the DeepSeek word analysis into
a committed asset so the app never has to call the LLM for enrichment at runtime.

Reads the 考研 word list from data/tuna.db, calls DeepSeek concurrently (with the
byte-stable system prefix so the prompt cache applies), and writes one enrichment
JSON object per line to assets/enrichment.jsonl. Resumable: already-done words are
skipped, so re-running continues where it left off.

    DEEPSEEK_API_KEY=... uv run scripts/enrich.py            # all words
    uv run scripts/enrich.py --limit 50                      # a slice

The key is read from $DEEPSEEK_API_KEY, else from tuna.toml [deepseek].api_key.
"""

import json
import os
import sqlite3
import sys
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed

import requests

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DB = os.path.join(ROOT, "data", "tuna.db")
OUT = os.path.join(ROOT, "assets", "enrichment.jsonl")
WORKERS = 8

# Byte-stable system prefix — MUST match the schema the Rust app deserializes.
SYSTEM_PROMPT = "你是考研英语词汇的词源拆解引擎。对给定的词输出一个严格符合 schema 的 json 对象。硬规则：①known_anchors 只用学习者可能已掌握的 CET-4 基础词；②词源必须诚实——真实词源标 etymology_confidence=solid，教学有用但非严格的俗词源标 folk，纯记忆钩子标 mnemonic，禁止编造词根；③derivation_zh 写成一条推导链『A + B → … → 词义』，像推公式，不要写成解释段落；④examples 两句，第一句用 CET-4 词汇改写，第二句贴近考研真题的学术书面风格并标 level=考研；⑤decomposable=false 时 morphemes 可为空，用 hook 兜底。schema = {\"word\":str,\"pos\":str,\"ipa\":str,\"gloss_zh\":str,\"freq_tier\":\"高频|中频|低频\",\"decomposable\":bool,\"morphemes\":[{\"unit\":str,\"type\":\"prefix|root|suffix\",\"meaning_zh\":str,\"gloss_en\":str,\"cognates\":[str]}],\"derivation_zh\":str,\"etymology_confidence\":\"solid|folk|mnemonic\",\"known_anchors\":[str],\"hook\":str,\"graph_edges\":[{\"target\":str,\"relation\":\"cognate_root|synonym|antonym|confusable\",\"via\":str,\"why_zh\":str}],\"collocations\":[str],\"examples\":[{\"en\":str,\"zh\":str,\"level\":str}],\"derive_puzzle\":{\"given_zh\":str,\"ask_zh\":str,\"answer_zh\":str}}"


def read_key():
    key = os.environ.get("DEEPSEEK_API_KEY")
    if key:
        return key
    try:
        import tomllib

        with open(os.path.join(ROOT, "tuna.toml"), "rb") as f:
            return tomllib.load(f).get("deepseek", {}).get("api_key", "")
    except Exception:
        return ""


def enrich_one(session, key, base, model, word):
    body = {
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {
                "role": "user",
                "content": f"word: {word}\n请只输出 json。",
            },
        ],
        "response_format": {"type": "json_object"},
        "temperature": 0.3,
        "max_tokens": 3200,
    }
    for attempt in range(3):
        try:
            r = session.post(
                f"{base}/chat/completions",
                headers={"Authorization": f"Bearer {key}"},
                json=body,
                timeout=120,
            )
            r.raise_for_status()
            content = r.json()["choices"][0]["message"]["content"]
            obj = json.loads(content)
            obj.setdefault("word", word)
            return obj
        except Exception as e:  # noqa: BLE001
            if attempt == 2:
                print(f"  ✗ {word}: {e}", file=sys.stderr, flush=True)
                return None
    return None


def main():
    limit = None
    if "--limit" in sys.argv:
        limit = int(sys.argv[sys.argv.index("--limit") + 1])

    key = read_key()
    if not key:
        print("no DeepSeek key ($DEEPSEEK_API_KEY or tuna.toml)", file=sys.stderr)
        return 2
    base = "https://api.deepseek.com"
    model = "deepseek-v4-flash"

    con = sqlite3.connect(DB)
    words = [r[0] for r in con.execute("SELECT word FROM dict ORDER BY priority ASC")]
    con.close()

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    done = set()
    if os.path.exists(OUT):
        with open(OUT, encoding="utf-8") as f:
            for line in f:
                try:
                    done.add(json.loads(line)["word"])
                except Exception:
                    pass
    todo = [w for w in words if w not in done]
    if limit:
        todo = todo[:limit]
    print(f"enrich: {len(todo)} to do, {len(done)} already baked", flush=True)

    lock = threading.Lock()
    session = requests.Session()
    ok = 0
    with open(OUT, "a", encoding="utf-8") as out, ThreadPoolExecutor(WORKERS) as pool:
        futures = {pool.submit(enrich_one, session, key, base, model, w): w for w in todo}
        for i, fut in enumerate(as_completed(futures)):
            word = futures[fut]
            obj = fut.result()
            if obj is None:
                continue
            with lock:
                out.write(json.dumps(obj, ensure_ascii=False) + "\n")
                out.flush()
                ok += 1
            if ok % 50 == 0 or i == len(todo) - 1:
                print(f"  [{ok}/{len(todo)}] latest: {word}", flush=True)
    print(f"\n✓ baked {ok}/{len(todo)} → {OUT}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
