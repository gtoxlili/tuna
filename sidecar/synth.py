# /// script
# requires-python = ">=3.10"
# dependencies = ["kokoro-onnx>=0.4.9", "soundfile>=0.12"]
# ///
"""Kokoro TTS batch sidecar for tuna.

Loads the ONNX model ONCE and synthesizes a batch of jobs, writing WAV files.
The deck is finite, so tuna pre-synthesizes offline and the TUI only ever plays
cached files — runtime latency is ~0 and study stays fully silent until you press
play (which is gated on the bound earphone).

Usage:  KOKORO_MODEL=… KOKORO_VOICES=… uv run sidecar/synth.py jobs.json
where jobs.json = [{"text": "...", "out": "/abs/path.wav", "voice": "af_heart", "speed": 1.0}, ...]
Already-existing out files are skipped. Progress prints one line per synth.
"""

import json
import os
import sys


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: synth.py jobs.json", file=sys.stderr)
        return 2
    model = os.environ.get("KOKORO_MODEL")
    voices = os.environ.get("KOKORO_VOICES")
    if not model or not voices or not os.path.exists(model) or not os.path.exists(voices):
        print(f"kokoro model/voices not found (KOKORO_MODEL={model}, KOKORO_VOICES={voices})", file=sys.stderr)
        return 3

    with open(sys.argv[1], encoding="utf-8") as f:
        jobs = json.load(f)
    todo = [j for j in jobs if not os.path.exists(j["out"])]
    print(f"synth: {len(todo)} to make, {len(jobs) - len(todo)} already cached", flush=True)
    if not todo:
        return 0

    import soundfile as sf
    from kokoro_onnx import Kokoro

    kok = Kokoro(model, voices)
    for i, j in enumerate(todo):
        samples, sr = kok.create(
            j["text"],
            voice=j.get("voice", "af_heart"),
            speed=float(j.get("speed", 1.0)),
            lang="en-us",
        )
        os.makedirs(os.path.dirname(j["out"]), exist_ok=True)
        sf.write(j["out"], samples, sr)
        print(f"[{i + 1}/{len(todo)}] {j['text'][:48]}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
