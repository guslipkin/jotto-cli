#!/usr/bin/env python3
"""Build data/coastlines.bin, the coastlines `flights` draws its map with.

Run by hand when the coastlines should change, not from build.rs: it downloads
Natural Earth's coastlines (public domain, https://www.naturalearthdata.com/)
at three scales, and the result (about 2 MB) is committed.

    python3 scripts/build-coastline-data.py

ratatui's own world map is 5,125 dots, too coarse to zoom into, so the game
draws Natural Earth's coastlines as lines instead, at the scale that suits the
view: 1:110m for the whole world, 1:50m for a region, 1:10m close in. Pass
`--geojson-dir` to use ne_{110m,50m,10m}_coastline.geojson already downloaded
(from https://github.com/nvkelso/natural-earth-vector, `geojson/`).

Points are stored as small steps from the one before, which keeps the 1:10m set
to 4 bytes a point: each scale has a resolution in steps per degree (1,000, or
about 110 m, for 1:110m and 1:50m, whose own accuracy is coarser; 10,000, about
11 m, for 1:10m). A step too long for 16 bits starts a new line there.

File format (little-endian):
    b"CST1", u8 number of scales, coarsest first; then for each scale:
        u32 steps per degree, u32 number of lines; then for each line:
            u32 points, i32 longitude and i32 latitude of the first (in steps),
            then (points - 1) pairs of i16 steps in longitude and latitude.
"""

import argparse
import json
import struct
import sys
import urllib.request
from pathlib import Path

BASE_URL = "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson"
# (scale, steps per degree), coarsest first; the game picks one by zoom.
SCALES = [("110m", 1_000), ("50m", 1_000), ("10m", 10_000)]
I16 = (-32_768, 32_767)


def load(name, geojson_dir):
    if geojson_dir:
        return json.loads((Path(geojson_dir) / name).read_text(encoding="utf-8"))
    url = f"{BASE_URL}/{name}"
    print(f"Downloading {url}", flush=True)
    req = urllib.request.Request(url, headers={"User-Agent": "jotto-cli coastline builder"})
    with urllib.request.urlopen(req, timeout=120) as resp:
        return json.load(resp)


def lines_of(geojson):
    for feature in geojson["features"]:
        geom = feature["geometry"]
        if geom["type"] == "LineString":
            yield geom["coordinates"]
        elif geom["type"] == "MultiLineString":
            yield from geom["coordinates"]


def encode(points, per_degree):
    """Lines of (first point, steps), split where a step won't fit 16 bits."""
    scaled = [(round(lon * per_degree), round(lat * per_degree)) for lon, lat, *_ in points]
    out = []
    current = [scaled[0]]
    for prev, point in zip(scaled, scaled[1:]):
        dx, dy = point[0] - prev[0], point[1] - prev[1]
        if not (I16[0] <= dx <= I16[1] and I16[0] <= dy <= I16[1]):
            out.append(current)
            current = [point]
            continue
        if dx or dy:
            current.append(point)
    out.append(current)
    return [line for line in out if len(line) >= 2]


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--geojson-dir", help="folder with ne_*_coastline.geojson")
    p.add_argument("--out", default="data/coastlines.bin")
    args = p.parse_args()

    body = bytearray(b"CST1")
    body += struct.pack("<B", len(SCALES))
    for scale, per_degree in SCALES:
        lines = []
        for points in lines_of(load(f"ne_{scale}_coastline.geojson", args.geojson_dir)):
            lines.extend(encode(points, per_degree))
        body += struct.pack("<II", per_degree, len(lines))
        for line in lines:
            body += struct.pack("<Iii", len(line), *line[0])
            for prev, point in zip(line, line[1:]):
                body += struct.pack("<hh", point[0] - prev[0], point[1] - prev[1])
        print(f"1:{scale}: {len(lines)} lines, {sum(map(len, lines)):,} points", flush=True)
    Path(args.out).write_bytes(body)
    print(f"Wrote {args.out} ({len(body):,} bytes)")


if __name__ == "__main__":
    sys.exit(main())
