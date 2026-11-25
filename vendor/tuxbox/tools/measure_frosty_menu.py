#!/usr/bin/env python3
"""Measure bright squares, green trails and background color in Frosty main-menu captures."""
import argparse
import hashlib
import json
from pathlib import Path

from PIL import Image


# Pixel coordinates at the comparison resolution. These areas exclude the
# menu, preview, settings button and default upper-left MangoHud overlay.
SIZE = (1280, 720)
REGIONS = ((360, 0, 1200, 72), (1216, 0, 1280, 690),
           (0, 200, 64, 560), (0, 576, 815, 690))


def bright_shapes(mask, width, height):
    """Find fully bright squares and four-connected, densely filled shapes."""
    previous = [0] * (width + 1)
    largest = 0
    bright = set()
    for y in range(height):
        row = [0] * (width + 1)
        for x in range(width):
            index = y * width + x
            if mask[index]:
                bright.add(index)
                row[x + 1] = 1 + min(previous[x], previous[x + 1], row[x])
                largest = max(largest, row[x + 1])
        previous = row

    boxes = 0
    while bright:
        queue = [bright.pop()]
        left, right, top, bottom, area = width, 0, height, 0, 0
        while queue:
            index = queue.pop()
            y, x = divmod(index, width)
            left, right = min(left, x), max(right, x)
            top, bottom = min(top, y), max(bottom, y)
            area += 1
            neighbors = [index - width, index + width]
            if x:
                neighbors.append(index - 1)
            if x + 1 < width:
                neighbors.append(index + 1)
            for neighbor in neighbors:
                if neighbor in bright:
                    bright.remove(neighbor)
                    queue.append(neighbor)
        w, h = right - left + 1, bottom - top + 1
        # A crop can turn a partial character into an apparently solid box.
        if left and top and right < width - 1 and bottom < height - 1:
            if min(w, h) >= 7 and area * 100 >= 90 * w * h:
                boxes += 1
    return largest, boxes


def measure(path):
    with Image.open(path) as source:
        # The desktop can clip a narrow strip from the bottom of a 16:9 window.
        # Preserve pixel aspect ratio and only sample the part shared by all captures.
        height = round(source.height * SIZE[0] / source.width)
        if source.format not in {"PNG", "JPEG", "WEBP"} or not 690 <= height <= SIZE[1]:
            raise ValueError(f"{path} must show at least the top 690/720 of a 16:9 game-window capture")
        image = source.convert("RGB").resize((SIZE[0], height), Image.Resampling.BOX)
    digest = hashlib.sha256()
    largest = boxes = green = pixels = 0
    chromatic = green_dominant = blue_dominant = 0
    red_sum = green_sum = blue_sum = 0
    for region in REGIONS:
        crop = image.crop(region)
        rgb = crop.tobytes()
        digest.update(rgb)
        mask = bytearray(crop.width * crop.height)
        for i, (r, g, b) in enumerate(zip(rgb[::3], rgb[1::3], rgb[2::3])):
            mask[i] = min(r, g, b) >= 230
            green += g > 40 and 10 * g > 13 * r and 25 * g > 24 * b
            # Exclude dark noise and nearly gray highlights. Keep teal samples
            # in the means even when the green and blue channels are tied.
            if max(r, g, b) >= 50 and max(r, g, b) - min(r, g, b) >= 30:
                chromatic += 1
                green_dominant += g > max(r, b)
                blue_dominant += b > max(r, g)
                red_sum += r
                green_sum += g
                blue_sum += b
        side, count = bright_shapes(mask, crop.width, crop.height)
        largest = max(largest, side)
        boxes += count
        pixels += len(mask)
    return dict(image=path.name, background_rgb_sha256=digest.hexdigest(),
                largest_solid_square_side_px=largest, bright_boxes=boxes,
                green_pixels=green, sampled_pixels=pixels, chromatic_pixels=chromatic,
                green_dominant_pixels=green_dominant, blue_dominant_pixels=blue_dominant,
                chromatic_mean_rgb=[round(channel / chromatic, 2) for channel in (red_sum, green_sum, blue_sum)]
                if chromatic else None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("frames", nargs="+", type=Path)
    args = parser.parse_args()
    rows = [measure(path) for path in args.frames]
    print(json.dumps(dict(resolution=SIZE, regions=REGIONS,
                          distinct_backgrounds=len({r["background_rgb_sha256"] for r in rows}),
                          frames=rows), indent=2))


if __name__ == "__main__":
    main()
