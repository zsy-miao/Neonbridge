#!/usr/bin/env python3
"""Compare aligned lossless SDR captures on CPU; no capture or GPU access.

PNG RGB/RGBA only. No resizing, registration, tonemapping, or frame matching
by similarity: these can hide rendering errors. Explicit ROI and thresholds
must be chosen before comparing. Pillow is an optional tool dependency.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path

try:
    from PIL import Image, ImageChops
except ImportError as exc:
    raise SystemExit("Pillow is required for frame comparison; install it in a dedicated Python environment.") from exc

MAX_PIXELS = 16_777_216
MAX_FRAMES = 256


def digest(data):
    return hashlib.sha256(data).hexdigest()


def load(path):
    with Image.open(path) as source:
        if source.format != "PNG" or source.mode not in ("RGB", "RGBA") or getattr(source,"n_frames",1)!=1:
            raise ValueError("expected single-frame 8-bit RGB/RGBA PNG")
        # Pillow can downconvert 16-bit RGB PNG; reject from the IHDR itself.
        with path.open("rb") as stream:
            header=stream.read(29)
        if len(header)!=29 or header[12:16]!=b"IHDR" or header[24]!=8 or header[25] not in (2,6):
            raise ValueError("expected 8-bit PNG")
        if source.width*source.height > MAX_PIXELS or not source.width or not source.height:
            raise ValueError("capture exceeds 16,777,216 pixels")
        if source.getexif().get(274,1)!=1:
            raise ValueError("orientation metadata must be normalized first")
        if "transparency" in source.info:
            raise ValueError("use explicit alpha rather than color-key transparency")
        metadata = {key:source.info.get(key) for key in ("icc_profile","gamma","srgb","chromaticity")}
        source.load()
        return source.copy(),metadata


def metrics(diff, tolerance):
    channels=diff.split()
    combined=channels[0]
    for channel in channels[1:]:
        combined=ImageChops.lighter(combined,channel)
    hist=combined.histogram()
    pixels=sum(hist)
    mask=combined.point([0 if i<=tolerance else 255 for i in range(256)])
    count=sum(hist[tolerance+1:])
    abs_sum=squares=0
    for band in channels:
        h=band.histogram()
        abs_sum+=sum(i*n for i,n in enumerate(h))
        squares+=sum(i*i*n for i,n in enumerate(h))
    samples=pixels*len(channels)
    mse=squares/samples
    return {"pixels":pixels,"changed_pixels":pixels-hist[0],
            "over_tolerance_pixels":count,"over_tolerance_fraction":count/pixels,
            "max_channel_error":combined.getextrema()[1],"mean_absolute_error":abs_sum/samples,
            "mse":mse,"psnr_db":10*math.log10(255**2/mse) if mse else None,
            "bounding_box":mask.getbbox()},combined,mask


def image_stats(image):
    r,g,b=image.split()[:3]
    maximum=ImageChops.lighter(ImageChops.lighter(r,g),b)
    minimum=ImageChops.darker(ImageChops.darker(r,g),b)
    return {"near_black_fraction":sum(maximum.histogram()[:9])/(image.width*image.height),
            "flat_rgb":all(ch.getextrema()[0]==ch.getextrema()[1] for ch in (r,g,b)),
            "max_rgb":maximum.getextrema()[1],"min_rgb":minimum.getextrema()[0],
            "pixel_sha256":digest(image.tobytes())}


def compare(reference,candidate,tolerance=0,max_fraction=0.0,max_tile_fraction=0.0,tile=64,roi=None,heatmap=None):
    if not 0<=tolerance<=255 or not 0<=max_fraction<=1 or not 0<=max_tile_fraction<=1 or not 16<=tile<=512:
        raise ValueError("invalid tolerance, fraction or tile size")
    a,meta_a=load(reference)
    b,meta_b=load(candidate)
    if a.size!=b.size or a.mode!=b.mode or meta_a!=meta_b:
        raise ValueError("capture dimensions, mode and color metadata must match")
    width,height=a.size
    if roi is None:
        roi=(0,0,width,height)
    x0,y0,x1,y1=roi
    if not (0<=x0<x1<=width and 0<=y0<y1<=height):
        raise ValueError("ROI outside capture")
    a,b=a.crop(roi),b.crop(roi)
    diff=ImageChops.difference(a,b)
    result,maximum,mask=metrics(diff,tolerance)
    box=result["bounding_box"]
    if box:
        result["bounding_box"]=[box[0]+x0,box[1]+y0,box[2]+x0,box[3]+y0]
    worst=[]
    max_local=0.0
    for y in range(0,a.height,tile):
        for x in range(0,a.width,tile):
            box=(x,y,min(x+tile,a.width),min(y+tile,a.height))
            crop=mask.crop(box)
            count=crop.histogram()[255]
            if not count:
                continue
            fraction=count/(crop.width*crop.height)
            max_local=max(max_local,fraction)
            worst.append({"box":[x+x0,y+y0,box[2]+x0,box[3]+y0],
                          "pixels_over_tolerance":count,"fraction":fraction,
                          "max_channel_error":maximum.crop(box).getextrema()[1]})
    worst.sort(key=lambda row:(-row["fraction"],-row["max_channel_error"],row["box"]))
    ref_stats,cand_stats=image_stats(a),image_stats(b)
    result.update({"reference":ref_stats,"candidate":cand_stats,"capture_size":[width,height],
                   "roi":list(roi),"mode":a.mode,"max_tile_fraction":max_local,
                   "changed_tiles":len(worst),"worst_tiles":worst[:10],
                   "passed":result["over_tolerance_fraction"]<=max_fraction and max_local<=max_tile_fraction})
    if heatmap:
        # Absolute error grayscale, not a normalized image that obscures scale.
        with heatmap.open("xb") as stream:
            maximum.save(stream,format="PNG")
    return result


def pairs(reference,candidate):
    if reference.is_file() and candidate.is_file():
        return [(reference.name,reference,candidate)]
    if not reference.is_dir() or not candidate.is_dir():
        raise ValueError("both inputs must be files or both directories")
    a={p.name:p for p in reference.iterdir() if p.is_file() and p.suffix.lower()==".png"}
    b={p.name:p for p in candidate.iterdir() if p.is_file() and p.suffix.lower()==".png"}
    if not a or a.keys()!=b.keys() or len(a)>MAX_FRAMES:
        raise ValueError("directories need identical nonempty PNG names, at most 256 frames")
    return [(name,a[name],b[name]) for name in sorted(a)]


def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference",type=Path)
    parser.add_argument("candidate",type=Path)
    parser.add_argument("--json",type=Path,required=True)
    parser.add_argument("--heatmaps",type=Path,help="fresh output directory")
    parser.add_argument("--tolerance",type=int,default=0)
    parser.add_argument("--max-fraction",type=float,default=0)
    parser.add_argument("--max-tile-fraction",type=float,default=0)
    parser.add_argument("--tile",type=int,default=64)
    parser.add_argument("--roi",nargs=4,type=int,metavar=("X0","Y0","X1","Y1"))
    args=parser.parse_args(argv)
    try:
        if args.json.exists():
            raise ValueError("report must be a fresh path")
        frame_pairs=pairs(args.reference,args.candidate)
        if args.heatmaps:
            args.heatmaps.mkdir(parents=True,exist_ok=False)
        rows=[]
        previous=None
        reference_repeats=candidate_repeats=0
        for i,(name,a,b) in enumerate(frame_pairs):
            row=compare(a,b,args.tolerance,args.max_fraction,args.max_tile_fraction,args.tile,
                        args.roi,args.heatmaps/f"{i:06d}.png" if args.heatmaps else None)
            hashes=(row["reference"]["pixel_sha256"],row["candidate"]["pixel_sha256"])
            if previous is not None:
                reference_repeats+=hashes[0]==previous[0]
                candidate_repeats+=hashes[1]==previous[1]
            previous=hashes
            rows.append({"frame":name,**row})
        first_bad = next((i for i, row in enumerate(rows) if not row["passed"]), None)
        first_black = next((i for i, row in enumerate(rows)
                            if row["candidate"]["near_black_fraction"] >= 0.99
                            and row["reference"]["near_black_fraction"] < 0.99), None)
        def location(i):
            if i is None:
                return None
            return {"index": i, "frame": rows[i]["frame"],
                    "previous_frame": rows[i-1]["frame"] if i else None,
                    "bounding_box": rows[i]["bounding_box"]}
        report={"schema":1,"scope":"Aligned SDR PNG pixel comparison; repeated/black frames are observations, not proof of freeze or a rendering bug.",
                "first_failed_frame": location(first_bad),
                "first_candidate_only_near_black_frame": location(first_black),
                "thresholds":{"channel_tolerance":args.tolerance,"max_fraction":args.max_fraction,
                              "max_tile_fraction":args.max_tile_fraction,"tile":args.tile},
                "frames":rows,"reference_adjacent_identical":reference_repeats,
                "candidate_adjacent_identical":candidate_repeats,
                "passed":all(row["passed"] for row in rows)}
        with args.json.open("x",encoding="utf-8") as stream:
            json.dump(report,stream,indent=2,allow_nan=False)
            stream.write("\n")
    except (OSError,ValueError,Image.DecompressionBombError) as exc:
        parser.error(str(exc))
    print(f"{sum(r['passed'] for r in rows)}/{len(rows)} frame comparisons passed")
    return int(not report["passed"])


if __name__=="__main__":
    raise SystemExit(main())
