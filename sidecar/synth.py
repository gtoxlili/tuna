# /// script
# requires-python = ">=3.10"
# dependencies = ["kokoro-onnx>=0.4.9", "soundfile>=0.12"]
# ///
"""Kokoro TTS sidecar for tuna — two modes.

--server (default use):  a warm, long-running server. Reads one JSON request per
    line from stdin, synthesizes, writes one JSON response line to stdout. The
    model loads once (lazily on first synth) and stays resident, so after the
    first ~6s cold start each clip is ~300ms. The TUI synthesizes ON DEMAND when
    you press play and the clip isn't cached — no offline pre-synth required.

<jobs.json>:  batch mode (optional bulk warm-up) — synth every job then exit.

Both write WAV. espeak-ng ships via espeakng-loader (no system install).
Env: KOKORO_MODEL, KOKORO_VOICES.
"""

import json
import os
import sys


def _synth(kok_holder, model, voices, job):
    """Synthesize one job to job['out']; return a response dict. Loads model lazily."""
    out = job["out"]
    if os.path.exists(out):
        return {"out": out, "ok": True, "cached": True}
    if kok_holder[0] is None:
        import soundfile as sf  # noqa: F401 (imported for side of availability)
        from kokoro_onnx import Kokoro

        kok_holder[0] = Kokoro(model, voices)
        kok_holder[1] = sf
    kok, sf = kok_holder[0], kok_holder[1]
    samples, sr = kok.create(
        job["text"],
        voice=job.get("voice", "af_heart"),
        speed=float(job.get("speed", 1.0)),
        lang="en-us",
    )
    os.makedirs(os.path.dirname(out), exist_ok=True)
    sf.write(out, samples, sr)
    return {"out": out, "ok": True}


def server(model, voices) -> int:
    kok_holder = [None, None]
    print(json.dumps({"ready": True}), flush=True)
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            job = json.loads(line)
            resp = _synth(kok_holder, model, voices, job)
        except Exception as e:  # noqa: BLE001 — report, keep serving
            resp = {"ok": False, "error": str(e)}
        print(json.dumps(resp), flush=True)
    return 0


def batch(model, voices, jobs_path) -> int:
    with open(jobs_path, encoding="utf-8") as f:
        jobs = json.load(f)
    todo = [j for j in jobs if not os.path.exists(j["out"])]
    print(f"synth: {len(todo)} to make, {len(jobs) - len(todo)} cached", flush=True)
    kok_holder = [None, None]
    for i, j in enumerate(todo):
        _synth(kok_holder, model, voices, j)
        print(f"[{i + 1}/{len(todo)}] {j['text'][:48]}", flush=True)
    return 0


def main() -> int:
    model = os.environ.get("KOKORO_MODEL")
    voices = os.environ.get("KOKORO_VOICES")
    if not model or not voices or not os.path.exists(model) or not os.path.exists(voices):
        print(f"kokoro model/voices not found (KOKORO_MODEL={model})", file=sys.stderr)
        return 3
    arg = sys.argv[1] if len(sys.argv) > 1 else "--server"
    if arg == "--server":
        return server(model, voices)
    return batch(model, voices, arg)


if __name__ == "__main__":
    raise SystemExit(main())
