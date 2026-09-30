#!/usr/bin/env python3
"""Extract the last production frame/status for compact documentation captures.

No model, pixel data, command names, or status text is synthesized. This removes
earlier frames and verbose conformance evidence from the recorded terminal stream.
"""
import argparse
import re
from pathlib import Path


def prepare(source: Path, marker: bytes) -> None:
    recorded = source.read_bytes()
    images: dict[bytes, list[bytes]] = {}
    for match in re.finditer(rb"\x1b_G(.*?)\x1b\\", recorded, re.DOTALL):
        command = match.group(1)
        if not command.startswith(b"a=T,t=d,f=24,s=800,v=480,"):
            continue
        image_id = re.search(rb",i=(\d+),", command)
        if image_id is None:
            continue
        images.setdefault(image_id.group(1), []).append(match.group(0))
    if not images:
        raise ValueError(f"No production viewport frame in {source}")
    matches = re.findall(re.escape(marker) + rb"[^\r\n\x1b]*", recorded)
    if not matches:
        raise ValueError(f"No expected application status in {source}")
    frame = next(reversed(images.values()))
    # Reset the presentation position, replay the unchanged graphics chunks, and
    # retain the exact application status text underneath. This is a component
    # capture, not an assertion about the live application's screen layout.
    output = b"\x1b[?1049h\x1b[2J\x1b[H\x1b[?25l"
    output += b"".join(frame) + b"\x1b[25;1H" + matches[-1] + b"\r\n"
    source.with_suffix(".capture.ansi").write_bytes(output)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("recordings", type=Path)
    args = parser.parse_args()
    for name, marker in (
        ("bracket", b"[selection-glyph] Acknowledgement"),
        ("palette", b"[outline] Command Palette"),
        ("preview", b"[dashed-outline] Preview:"),
    ):
        prepare(args.recordings / f"{name}.ansi", marker)
    print(f"Prepared compact production frame/status captures in {args.recordings}")


if __name__ == "__main__":
    main()
