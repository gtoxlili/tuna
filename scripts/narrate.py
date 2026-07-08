# /// script
# requires-python = ">=3.11"
# dependencies = ["requests>=2.31"]
# ///
"""tuna grounded bake — STAGE 2: canonical clustering + the ONE caged LLM call.

Reads data/etym_cache.jsonl (stage 1's verified etymologies) and produces the
committed assets:
  • assets/morphemes.jsonl — canonical morpheme nodes (surface, kind, gloss_en,
    specificity = IDF over the deck). The human/curation review surface.
  • assets/enrichment.jsonl — per-word content in the Rust Enrichment shape, with
    morphemes taken VERBATIM from Wiktionary (the cage) and only their zh glosses,
    the derivation chain, examples, and edges authored by DeepSeek.

The LLM is handed the verified morphemes as immutable ground truth. It may NOT add,
drop, or rename a morpheme — a fabricated root is structurally unrepresentable
because the stored morphemes are the Wiktionary ones, not the model's. Confidence
is set by the PIPELINE from the stage-1 category, never by the model.

    DEEPSEEK_API_KEY=… uv run scripts/narrate.py --limit 12   # sample
    DEEPSEEK_API_KEY=… uv run scripts/narrate.py              # full bake
"""

import json
import math
import os
import re
import sys
import threading
from collections import Counter
from concurrent.futures import ThreadPoolExecutor, as_completed

import requests

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CACHE = os.path.join(ROOT, "data", "etym_cache.jsonl")
MORPH_OUT = os.path.join(ROOT, "assets", "morphemes.jsonl")
ENRICH_OUT = os.path.join(ROOT, "assets", "enrichment.jsonl")
BASE = "https://api.deepseek.com"
MODEL = "deepseek-v4-flash"
WORKERS = 8

CITED = {"cited-affix", "cited-1hop"}

SYSTEM = (
    "你是考研英语词汇的词素讲解引擎。会给你一个词、它的类别、以及它来自 Wiktionary 的**已核验真实词素或词源**。"
    "你只做讲解,绝不改动词源事实。铁律:①绝不新增、删除或替换给定的词素/词根,也绝不编造任何词根;"
    "②对 cited 类:为每个给定词素写准确的中文释义(依据给定英文义),再写一条推导链 derivation_zh『甲+乙→…→今义』,像推公式;"
    "③对 single-root 类:只讲这个被引用词根的含义与它串起的同根直觉,不要硬拆成多个词素;"
    "④对 germanic 类:decomposable=false,morphemes 留空,给一个诚实、不牵强的记忆钩子 hook,绝不编词根;"
    "⑤examples 两句,第一句用 CET-4 词汇改写,第二句贴近考研真题的学术书面风格并标 level=考研;"
    "⑥可给 1-2 个真实的易混/近义词到 edges(relation=confusable|synonym,附 why_zh)。"
    "只输出 json,schema={\"gloss_zh\":str,\"morpheme_zh\":[{\"surface\":str,\"meaning_zh\":str}],"
    "\"derivation_zh\":str,\"root_zh\":str,\"decomposable\":bool,\"hook\":str,"
    "\"edges\":[{\"target\":str,\"relation\":str,\"why_zh\":str}],"
    "\"examples\":[{\"en\":str,\"zh\":str,\"level\":str}]}。morpheme_zh 里的 surface 必须与给定词素完全一致。"
)


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


def norm(surface):
    return surface.strip().strip("-— ").lower()


def build_morphemes(entries):
    """Deterministic clustering by normalized surface → canonical nodes + IDF."""
    members = {}  # id -> set(words)
    meta = {}     # id -> (surface, kind, gloss_en)
    for e in entries:
        for m in e.get("morphemes", []):
            mid = norm(m["surface"])
            if not mid or len(mid) < 2:
                continue
            members.setdefault(mid, set()).add(e["word"])
            if mid not in meta or (not meta[mid][2] and m.get("gloss_en")):
                meta[mid] = (m["surface"], m.get("kind", ""), m.get("gloss_en", ""))
    nodes = {}
    for mid, ws in members.items():
        surface, kind, gloss_en = meta[mid]
        nodes[mid] = {
            "id": mid, "surface": surface, "kind": kind, "gloss_en": gloss_en,
            "member_count": len(ws),
            "specificity": round(1.0 / math.log(1 + len(ws)), 4),
        }
    return nodes


def call(session, key, entry):
    word, cat = entry["word"], entry["category"]
    if cat in CITED:
        ms = ", ".join(f"{m['surface']}({m.get('gloss_en','')})" for m in entry["morphemes"])
        ctx = f"类别: {cat}\n已核验词素: [{ms}]"
    elif cat == "single-root":
        ctx = f"类别: single-root\n已核验词源: {entry.get('src_lang','la')} {entry.get('etymon','')}"
    else:
        ctx = "类别: germanic(无清晰词源分解)"
    body = {
        "model": MODEL,
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": f"word: {word}\n{ctx}\n请只输出 json。"},
        ],
        "response_format": {"type": "json_object"},
        "temperature": 0.3, "max_tokens": 1600,
    }
    for attempt in range(3):
        try:
            r = session.post(f"{BASE}/chat/completions",
                             headers={"Authorization": f"Bearer {key}"}, json=body, timeout=120)
            r.raise_for_status()
            return json.loads(r.json()["choices"][0]["message"]["content"])
        except Exception as e:  # noqa: BLE001
            if attempt == 2:
                print(f"  ✗ {word}: {e}", file=sys.stderr, flush=True)
                return None
    return None


def assemble(entry, llm):
    """Merge VERIFIED morphemes (Wiktionary) with the LLM's zh glosses + narration.
    The morpheme surfaces are the cage — taken from Wiktionary, never the model."""
    word, cat = entry["word"], entry["category"]
    zh_list = llm.get("morpheme_zh", [])
    zh_map = {norm(z.get("surface", "")): z.get("meaning_zh", "") for z in zh_list}
    morphemes = []
    if cat in CITED:
        for i, m in enumerate(entry["morphemes"]):
            meaning = zh_map.get(norm(m["surface"]), "")
            if not meaning and i < len(zh_list):  # positional fallback
                meaning = zh_list[i].get("meaning_zh", "")
            morphemes.append({
                "unit": m["surface"], "type": m.get("kind", ""),
                "meaning_zh": meaning, "gloss_en": m.get("gloss_en", ""), "cognates": [],
            })
    elif cat == "single-root" and entry.get("etymon"):
        # Anchor the cited root as a single morpheme node so cognates sibling up.
        morphemes.append({
            "unit": entry["etymon"], "type": "root",
            "meaning_zh": llm.get("root_zh", "") or llm.get("gloss_zh", ""),
            "gloss_en": entry.get("src_lang", ""), "cognates": [],
        })
    confidence = {"cited-affix": "cited", "cited-1hop": "cited",
                  "single-root": "cited-root"}.get(cat, "mnemonic")
    citation = {"rev_id": entry.get("rev_id"), "category": cat}
    if entry.get("src_rev_id"):
        citation["src"] = entry.get("src")
        citation["src_rev_id"] = entry.get("src_rev_id")
    return {
        "word": word,
        "gloss_zh": llm.get("gloss_zh", ""),
        "freq_tier": "",
        "decomposable": bool(morphemes) if cat in CITED else False,
        "morphemes": morphemes,
        "derivation_zh": llm.get("derivation_zh", "") or llm.get("root_zh", ""),
        "etymology_confidence": confidence,
        "hook": llm.get("hook", ""),
        "graph_edges": [
            {"target": ed.get("target", ""), "relation": ed.get("relation", ""),
             "via": "", "why_zh": ed.get("why_zh", "")}
            for ed in llm.get("edges", []) if ed.get("target")
        ],
        "collocations": [],
        "examples": llm.get("examples", []),
        "citation": citation,
    }


def main():
    limit = int(sys.argv[sys.argv.index("--limit") + 1]) if "--limit" in sys.argv else None
    key = read_key()
    if not key:
        print("no DeepSeek key", file=sys.stderr)
        return 2

    entries = {}
    with open(CACHE, encoding="utf-8") as f:
        for line in f:
            try:
                o = json.loads(line)
            except Exception:
                continue
            if o.get("word") and o.get("category"):
                entries[o["word"]] = o
    entries = list(entries.values())

    # ── S4a: canonical morpheme nodes (deterministic) ──
    nodes = build_morphemes([e for e in entries if e["category"] in CITED])
    with open(MORPH_OUT, "w", encoding="utf-8") as f:
        for n in sorted(nodes.values(), key=lambda x: -x["member_count"]):
            f.write(json.dumps(n, ensure_ascii=False) + "\n")
    print(f"S4a: {len(nodes)} canonical morpheme nodes → {MORPH_OUT}", flush=True)

    # ── S4b: caged LLM narration ──
    todo = [e for e in entries if e["category"] in CITED or e["category"] in ("single-root", "germanic")]
    done = set()
    if os.path.exists(ENRICH_OUT):
        with open(ENRICH_OUT, encoding="utf-8") as f:
            for line in f:
                try:
                    done.add(json.loads(line)["word"])
                except Exception:
                    pass
    todo = [e for e in todo if e["word"] not in done]
    if limit:
        todo = todo[:limit]
    print(f"S4b: narrating {len(todo)} words ({len(done)} done)", flush=True)

    lock = threading.Lock()
    session = requests.Session()
    ok = 0
    cats = Counter()
    with open(ENRICH_OUT, "a", encoding="utf-8") as out, ThreadPoolExecutor(WORKERS) as pool:
        futs = {pool.submit(call, session, key, e): e for e in todo}
        for i, fut in enumerate(as_completed(futs)):
            e = futs[fut]
            llm = fut.result()
            if llm is None:
                continue
            rec = assemble(e, llm)
            with lock:
                out.write(json.dumps(rec, ensure_ascii=False) + "\n"); out.flush()
                ok += 1
                cats[e["category"]] += 1
            if ok % 100 == 0:
                print(f"  … {ok}/{len(todo)}", flush=True)
    print(f"\n✓ narrated {ok}/{len(todo)} → {ENRICH_OUT}  by-cat {dict(cats)}", flush=True)


if __name__ == "__main__":
    raise SystemExit(main())
