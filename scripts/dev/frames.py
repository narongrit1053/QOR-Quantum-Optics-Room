"""Summarises viewport frames in a LabSetup log (RUST_LOG=lab_viewport=debug) after a moment.

    python frames.py <stderr log> <start HH:MM:SS.fff UTC> [<end HH:MM:SS.fff UTC>]

Prints the frame count, the time of the last frame after the start, the largest gap between
frames and the render scales seen. Use it to check that rendering stops after an action.
"""
import re
import sys
from datetime import datetime

FRAME = re.compile(r"T(\d\d:\d\d:\d\d\.\d+)Z DEBUG lab_viewport::host: viewport frame (\d+) rendered at (\d+x\d+) \(scale ([\d.]+)\)")


def secs(hms: str) -> float:
    t = datetime.strptime(hms[:15], "%H:%M:%S.%f")
    return t.hour * 3600 + t.minute * 60 + t.second + t.microsecond / 1e6


def main() -> None:
    log, start = sys.argv[1], secs(sys.argv[2])
    end = secs(sys.argv[3]) if len(sys.argv) > 3 else float("inf")
    frames = []
    with open(log, encoding="utf-8", errors="replace") as f:
        for line in f:
            m = FRAME.search(line)
            if m and start <= secs(m.group(1)) < end:
                frames.append((secs(m.group(1)), int(m.group(2)), m.group(3), m.group(4)))
    if not frames:
        print("no frames")
        return
    times = [start] + [t for t, *_ in frames]
    gaps = [round((b - a) * 1000) for a, b in zip(times, times[1:])]
    print(f"frames={len(frames)} (#{frames[0][1]}..#{frames[-1][1]}) first +{gaps[0]} ms, "
          f"last +{round((frames[-1][0] - start) * 1000)} ms, max gap {max(gaps)} ms, "
          f"scales {sorted({s for *_, s in frames})}, sizes {sorted({z for _, _, z, _ in frames})}")
    print("gaps ms:", gaps)


if __name__ == "__main__":
    main()
