# /// script
# requires-python = ">=3.11"
# dependencies = ["requests>=2.31"]
# ///
"""tuna grounded-etymology bake — STAGE 1: the coverage mirror (deterministic, no LLM).

For each 考研 word: fetch its Wiktionary etymology (pinning rev_id), parse the REAL
morpheme templates ({{affix}}/{{prefix}}/{{suffix}}/{{compound}}/{{root}}/{{der}}),
follow ONE hop to a Latin/Greek source page when the English page only points there
(circumscribe → circumscrībō), and CLASSIFY. It reports the true decomposable rate
and the needs_review tail BEFORE a cent is spent on the caged LLM narration (stage 2).

Every parsed etymology is cached to assets/etym_cache.jsonl (resumable, and reused by
stage 2). Confidence is COMPUTED here from evidence, never guessed:
  cited-affix   — a real affix template gives ≥2 morphemes (en, or 1-hop la/grc)
  single-root   — a real etymon but borrowed whole, no multi-part split
  germanic      — inherited Germanic core word, no clean affix
  no-etymology  — Wiktionary has no etymology section
  needs-review  — template present but unparseable (nested/odd) — a human/agent must look

    uv run scripts/bake.py            # all words
    uv run scripts/bake.py --limit 40 # a sample to preview the terrain
"""

import json
import os
import re
import sqlite3
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed

import requests

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DB = os.path.join(ROOT, "data", "tuna.db")
CACHE = os.path.join(ROOT, "data", "etym_cache.jsonl")  # gitignored intermediate
UA = "tuna-etymology/0.1 (personal study tool; gtoxlili@outlook.com)"
API = "https://en.wiktionary.org/w/api.php"
WORKERS = 6

AFFIX_TMPL = {"af", "affix", "prefix", "suffix", "compound", "com", "con", "confix", "surf"}
ETYMON_TMPL = {"der", "bor", "inh", "inh+", "der+", "bor+"}
LATINATE = {"la", "grc", "grc-koi", "LL.", "ML.", "la-med"}


def fetch_wikitext(session, title):
    """Return (wikitext, rev_id) for a page title, or (None, None)."""
    r = session.get(
        API,
        params={
            "action": "parse", "page": title, "prop": "wikitext|revid",
            "format": "json", "formatversion": "2", "redirects": "1",
        },
        headers={"User-Agent": UA},
        timeout=40,
    )
    if r.status_code != 200:
        return None, None
    d = r.json().get("parse", {})
    return d.get("wikitext"), d.get("revid")


def section(wt, lang_header):
    """Extract a top-level language section's wikitext (== English == … before ----)."""
    m = re.search(rf"==\s*{lang_header}\s*==(.*?)(?:\n----|\Z)", wt, re.S)
    return m.group(1) if m else ""


def etymology_block(sec):
    """All Etymology subsections concatenated."""
    blocks = re.findall(r"===+\s*Etymology[^=]*===+(.*?)(?=\n===|\Z)", sec, re.S)
    return "\n".join(blocks) if blocks else ""


def templates(text):
    """Top-level {{name|args}} with no nested braces in args (nested ⇒ skipped/needs-review)."""
    return re.findall(r"\{\{([a-z+]+)\s*\|([^{}]*)\}\}", text)


def part_surface_gloss(part):
    """'trāns-<t:across>' → ('trāns-', 'across'). Strips inline <..> modifiers."""
    gloss = ""
    gm = re.search(r"<t:([^>]*)>", part) or re.search(r"<gloss:([^>]*)>", part)
    if gm:
        gloss = gm.group(1).strip()
    surface = re.split(r"<", part, maxsplit=1)[0].strip()
    return surface, gloss


def parse_affix(tmpls):
    """First affix template with ≥2 real parts → [(surface, gloss), …], else None."""
    for name, args in tmpls:
        if name not in AFFIX_TMPL:
            continue
        parts = [p.strip() for p in args.split("|")]
        # drop the leading lang code and any nocat=/pos=/lang= kv args
        morphs = []
        for p in parts[1:]:
            if not p or "=" in p.split("<")[0]:
                continue
            s, g = part_surface_gloss(p)
            if s and s not in ("-", ""):
                morphs.append({"surface": s, "gloss_en": g})
        if len(morphs) >= 2:
            return morphs, name
    return None, None


def find_etymon(tmpls):
    """First {{der/bor/inh|en|LANG|WORD}} → (lang, word)."""
    for name, args in tmpls:
        if name not in ETYMON_TMPL:
            continue
        parts = [p.strip() for p in args.split("|")]
        if len(parts) >= 3 and parts[0] == "en":
            lang, word = parts[1], part_surface_gloss(parts[2])[0]
            if word:
                return lang, word
    return None


def classify(session, word):
    wt, rev = fetch_wikitext(session, word)
    if not wt:
        return {"word": word, "category": "no-page"}
    eng = etymology_block(section(wt, "English"))
    if not eng.strip():
        return {"word": word, "category": "no-etymology", "rev_id": rev}
    tmpls = templates(eng)
    morphs, via = parse_affix(tmpls)
    if morphs:
        return {"word": word, "category": "cited-affix", "hop": 0, "via_tmpl": via,
                "morphemes": morphs, "rev_id": rev}
    etymon = find_etymon(tmpls)
    if etymon and etymon[0] in LATINATE:
        lang, src = etymon
        wt2, rev2 = fetch_wikitext(session, src)
        if wt2:
            langname = {"la": "Latin", "grc": "Ancient Greek"}.get(lang, lang)
            sec2 = etymology_block(section(wt2, langname))
            m2, via2 = parse_affix(templates(sec2))
            if m2:
                return {"word": word, "category": "cited-1hop", "hop": 1, "src": src,
                        "src_lang": lang, "via_tmpl": via2, "morphemes": m2,
                        "rev_id": rev, "src_rev_id": rev2}
        return {"word": word, "category": "single-root", "etymon": src, "src_lang": lang,
                "rev_id": rev}
    if any(n in ("inh", "inh+") for n, _ in tmpls):
        return {"word": word, "category": "germanic", "rev_id": rev}
    if tmpls:
        return {"word": word, "category": "needs-review", "rev_id": rev}
    return {"word": word, "category": "no-templates", "rev_id": rev}


def main():
    limit = int(sys.argv[sys.argv.index("--limit") + 1]) if "--limit" in sys.argv else None
    con = sqlite3.connect(DB)
    words = [r[0] for r in con.execute("SELECT word FROM dict ORDER BY priority ASC")]
    con.close()

    done = {}
    if os.path.exists(CACHE):
        with open(CACHE, encoding="utf-8") as f:
            for line in f:
                try:
                    o = json.loads(line); done[o["word"]] = o
                except Exception:
                    pass
    todo = [w for w in words if w not in done]
    if limit:
        todo = todo[:limit]
    print(f"coverage: {len(todo)} to fetch, {len(done)} cached", flush=True)

    lock = threading.Lock()
    session = requests.Session()
    results = list(done.values())
    t0 = time.time()
    with open(CACHE, "a", encoding="utf-8") as out, ThreadPoolExecutor(WORKERS) as pool:
        futs = {pool.submit(classify, session, w): w for w in todo}
        for i, fut in enumerate(as_completed(futs)):
            try:
                res = fut.result()
            except Exception as e:  # noqa: BLE001
                res = {"word": futs[fut], "category": "error", "err": str(e)}
            with lock:
                out.write(json.dumps(res, ensure_ascii=False) + "\n"); out.flush()
                results.append(res)
            if (i + 1) % 100 == 0:
                print(f"  … {i + 1}/{len(todo)} ({(i+1)/(time.time()-t0):.1f}/s)", flush=True)

    # ── the mirror ──
    from collections import Counter
    cats = Counter(r["category"] for r in results)
    total = len(results)
    print(f"\n═══ COVERAGE ({total} words) ═══")
    order = ["cited-affix", "cited-1hop", "single-root", "germanic",
             "no-etymology", "no-templates", "needs-review", "no-page", "error"]
    decomposable = cats["cited-affix"] + cats["cited-1hop"]
    for c in order:
        if cats.get(c):
            bar = "█" * round(40 * cats[c] / total)
            print(f"  {c:14} {cats[c]:5}  {100*cats[c]/total:5.1f}%  {bar}")
    print(f"\n  DECOMPOSABLE (cited): {decomposable}/{total} = {100*decomposable/total:.1f}%")
    print(f"  needs-review tail:    {cats.get('needs-review',0)}")
    # a few real examples
    print("\n  samples:")
    for c in ("cited-affix", "cited-1hop", "single-root", "germanic"):
        ex = next((r for r in results if r["category"] == c), None)
        if ex:
            ms = " + ".join(f"{m['surface']}({m['gloss_en']})" for m in ex.get("morphemes", [])) or ex.get("etymon", "")
            print(f"    {c:12} {ex['word']:14} {ms}")


if __name__ == "__main__":
    raise SystemExit(main())
