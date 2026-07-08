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
WORKERS = 3  # Wikimedia API etiquette — low concurrency, retry on throttle

# A word we should NOT re-fetch on resume (i.e. it succeeded). Failed fetches
# (no-page/error) are retried so a throttled run heals itself on re-run.
GOOD = {
    "cited-affix", "cited-1hop", "single-root", "germanic",
    "no-etymology", "no-templates", "needs-review",
}

AFFIX_TMPL = {"af", "affix", "prefix", "suffix", "compound", "com", "con", "confix", "surf"}
# Wiktionary's newer unified {{ety|LANG|:SUBTYPE|...}} wrapper (politic -al lives here).
ETY_AFFIX_SUB = {":af", ":affix", ":prefix", ":suffix", ":compound", ":com", ":con", ":confix", ":surf"}
ETY_DER_SUB = {":der", ":bor", ":inh", ":uder", ":der+", ":bor+"}
ETYMON_TMPL = {"der", "bor", "inh", "inh+", "der+", "bor+", "uder", "ubor"}
LATINATE = {"la", "grc", "grc-koi", "grc-gre", "la-med", "la-ecc", "LL.", "ML."}


def fetch_wikitext(session, title):
    """Return (wikitext, rev_id) for a page title, or (None, None). Retries on throttle."""
    params = {
        "action": "parse", "page": title, "prop": "wikitext|revid",
        "format": "json", "formatversion": "2", "redirects": "1", "maxlag": "5",
    }
    for attempt in range(5):
        try:
            r = session.get(API, params=params, headers={"User-Agent": UA}, timeout=40)
            if r.status_code == 200 and "Retry-After" not in r.headers:
                d = r.json().get("parse", {})
                if d:
                    return d.get("wikitext"), d.get("revid")
                return None, None  # genuine missingtitle / no parse
            # throttled (429/503) or maxlag — back off and retry
            wait = float(r.headers.get("Retry-After", 0) or 2 ** attempt)
            time.sleep(min(max(wait, 1.0), 30.0))
        except Exception:
            time.sleep(2 ** attempt)
    return None, None


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


def _morphs_from_parts(parts):
    morphs = []
    for p in parts:
        if not p or "=" in p.split("<")[0]:
            continue
        s, g = part_surface_gloss(p)
        if s and s not in ("-", ""):
            morphs.append({"surface": s, "gloss_en": g})
    return morphs


def parse_affix(tmpls):
    """First affix decomposition with ≥2 real parts → (morphemes, via), else (None, None).
    Handles both bare {{af|...}} and the {{ety|LANG|:af|...}} wrapper format."""
    for name, args in tmpls:
        parts = [p.strip() for p in args.split("|")]
        if name in AFFIX_TMPL:
            morph_parts = parts[1:]  # skip lang
        elif name == "ety" and len(parts) >= 2 and parts[1] in ETY_AFFIX_SUB:
            morph_parts = parts[2:]  # skip lang + :subtype
        else:
            continue
        morphs = _morphs_from_parts(morph_parts)
        if len(morphs) >= 2:
            return morphs, (name if name != "ety" else parts[1].lstrip(":"))
    return None, None


def find_etymon(tmpls):
    """All etymon templates → prefer the Latinate one (president goes fro→la).
    Handles {{der/bor/inh/uder}} and the {{ety|LANG|:der|SRCLANG|WORD}} wrapper."""
    cands = []
    for name, args in tmpls:
        parts = [p.strip() for p in args.split("|")]
        if name in ETYMON_TMPL and len(parts) >= 3 and parts[0] == "en":
            cands.append((parts[1], part_surface_gloss(parts[2])[0]))
        elif name == "ety" and len(parts) >= 4 and parts[1] in ETY_DER_SUB and parts[2]:
            cands.append((parts[2], part_surface_gloss(parts[3])[0]))
    for lang, word in cands:
        if lang in LATINATE and word:
            return lang, word
    for lang, word in cands:
        if word:
            return lang, word
    return None


def classify_raw(word, eng, rev, src_ety=None, src=None, src_lang=None, src_rev=None):
    """S1-S3 parse from RAW etymology text — deterministic, offline, re-runnable."""
    base = {"word": word, "rev_id": rev, "ety": eng[:2000]}
    if src_ety is not None:
        base.update({"src_ety": src_ety[:2000], "src": src, "src_lang": src_lang, "src_rev_id": src_rev})
    if not eng.strip():
        return {**base, "category": "no-etymology"}
    tmpls = templates(eng)
    morphs, via = parse_affix(tmpls)
    if morphs:
        return {**base, "category": "cited-affix", "hop": 0, "via_tmpl": via, "morphemes": morphs}
    # 1-hop: parse the (already-fetched) Latinate source page
    if src_ety:
        m2, via2 = parse_affix(templates(src_ety))
        if m2:
            return {**base, "category": "cited-1hop", "hop": 1, "via_tmpl": via2, "morphemes": m2}
    etymon = find_etymon(tmpls)
    if etymon and etymon[0] in LATINATE:
        return {**base, "category": "single-root", "etymon": etymon[1], "src_lang": etymon[0]}
    if any(n in ("inh", "inh+", "uder") for n, _ in tmpls) or (etymon and etymon[0] in ("enm", "ang", "gem-pro", "non", "fro", "frm")):
        return {**base, "category": "germanic"}
    if tmpls:
        return {**base, "category": "needs-review"}
    return {**base, "category": "no-templates"}


def classify(session, word):
    wt, rev = fetch_wikitext(session, word)
    if not wt:
        return {"word": word, "category": "no-page"}
    eng = etymology_block(section(wt, "English"))
    tmpls = templates(eng)
    # Fetch the Latinate source page up front (for 1-hop) if the English page has no affix.
    if not parse_affix(tmpls)[0]:
        etymon = find_etymon(tmpls)
        if etymon and etymon[0] in LATINATE:
            lang, src = etymon
            wt2, rev2 = fetch_wikitext(session, src)
            if wt2:
                langname = {"la": "Latin", "grc": "Ancient Greek"}.get(lang, "Latin")
                sec2 = etymology_block(section(wt2, langname)) or etymology_block(section(wt2, "Latin"))
                return classify_raw(word, eng, rev, src_ety=sec2, src=src, src_lang=lang, src_rev=rev2)
    return classify_raw(word, eng, rev)


def main():
    limit = int(sys.argv[sys.argv.index("--limit") + 1]) if "--limit" in sys.argv else None
    con = sqlite3.connect(DB)
    words = [r[0] for r in con.execute("SELECT word FROM dict ORDER BY priority ASC")]
    con.close()

    # Load cache keeping the BEST result per word (a GOOD category beats a failed fetch).
    cached = {}
    if os.path.exists(CACHE):
        with open(CACHE, encoding="utf-8") as f:
            for line in f:
                try:
                    o = json.loads(line)
                except Exception:
                    continue
                w = o.get("word")
                if not w:
                    continue
                prev = cached.get(w)
                if prev is None or (o["category"] in GOOD and prev["category"] not in GOOD):
                    cached[w] = o
    # A word is "done" only if it succeeded AND we stored its raw etymology (so
    # parser upgrades force a one-time re-fetch, then re-parse offline forever).
    done = {w for w, o in cached.items() if o["category"] in GOOD and o.get("ety") is not None}
    todo = [w for w in words if w not in done]
    if limit:
        todo = todo[:limit]
    print(f"coverage: {len(todo)} to fetch, {len(done)} good cached", flush=True)

    lock = threading.Lock()
    session = requests.Session()
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
                cached[res["word"]] = res
            if (i + 1) % 100 == 0:
                print(f"  … {i + 1}/{len(todo)} ({(i+1)/(time.time()-t0):.1f}/s)", flush=True)

    # Rewrite the cache deduped (best per word).
    with open(CACHE, "w", encoding="utf-8") as f:
        for o in cached.values():
            f.write(json.dumps(o, ensure_ascii=False) + "\n")
    report(list(cached.values()))


def report(results):
    from collections import Counter
    cats = Counter(r["category"] for r in results)
    total = len(results) or 1
    print(f"\n═══ COVERAGE ({total} words) ═══")
    order = ["cited-affix", "cited-1hop", "single-root", "germanic",
             "no-etymology", "no-templates", "needs-review", "no-page", "error"]
    cited = cats["cited-affix"] + cats["cited-1hop"]
    with_etymon = cited + cats["single-root"]  # LLM can decompose the cited etymon
    for c in order:
        if cats.get(c):
            bar = "█" * round(40 * cats[c] / total)
            print(f"  {c:14} {cats[c]:5}  {100*cats[c]/total:5.1f}%  {bar}")
    print(f"\n  DETERMINISTIC decomposable (cited): {cited}/{total} = {100*cited/total:.1f}%")
    print(f"  + single-root (LLM decomposes the cited etymon): {100*with_etymon/total:.1f}% grounded total")
    print(f"  germanic (honest non-decomposable): {cats.get('germanic',0)}")
    print(f"  needs-review tail: {cats.get('needs-review',0)}")
    print("\n  samples:")
    for c in ("cited-affix", "cited-1hop", "single-root", "germanic"):
        ex = next((r for r in results if r["category"] == c), None)
        if ex:
            ms = " + ".join(f"{m['surface']}({m['gloss_en']})" for m in ex.get("morphemes", [])) or ex.get("etymon", "")
            print(f"    {c:12} {ex['word']:14} {ms}")


def reclassify():
    """Offline: re-parse every cached word from its stored raw etymology (no fetch)."""
    cached = {}
    with open(CACHE, encoding="utf-8") as f:
        for line in f:
            try:
                o = json.loads(line)
            except Exception:
                continue
            if o.get("word"):
                cached[o["word"]] = o
    out = {}
    for w, o in cached.items():
        if o.get("ety") is not None:
            out[w] = classify_raw(w, o["ety"], o.get("rev_id"), o.get("src_ety"),
                                  o.get("src"), o.get("src_lang"), o.get("src_rev_id"))
        else:
            out[w] = o
    with open(CACHE, "w", encoding="utf-8") as f:
        for o in out.values():
            f.write(json.dumps(o, ensure_ascii=False) + "\n")
    print(f"reclassified {len(out)} cached words (offline)")
    report(list(out.values()))


if __name__ == "__main__" and "--reclassify" in sys.argv:
    reclassify()
    raise SystemExit(0)

if __name__ == "__main__":
    raise SystemExit(main())
