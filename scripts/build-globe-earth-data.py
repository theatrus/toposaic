#!/usr/bin/env python3
"""Pack a reproducible Whole Earth raster with its print classes."""

import argparse
import csv
import math
import struct
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

def print_class(red, green, blue):
    if blue - green >= 13 and green - red >= 13:
        return 3  # water
    if red >= 200 and blue >= red + 2 and abs(green - red) < 18:
        return 2  # permanent ice and snow
    if green - red >= 9 and green - blue >= 8:
        return 1  # forest and dense vegetation
    if green >= red - 6 and green - blue >= 22:
        return 4  # grass, savanna, and other open vegetation
    return 0  # bare land and rock


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elevation', type=Path, help='NOAA ERDDAP CSV described in assets/globe/README.md')
    parser.add_argument('land_cover', type=Path, help='RGB bytes in north-to-south order at the requested grid resolution')
    parser.add_argument('output', type=Path)
    parser.add_argument('--arcminutes', type=int, choices=[15, 30], default=30)
    args = parser.parse_args()
    width, height = 21600 // args.arcminutes, 10800 // args.arcminutes
    step = args.arcminutes / 60
    offset = ((args.arcminutes - 1) // 2 + 0.5) / 60
    rgb = args.land_cover.read_bytes()
    if len(rgb) != width * height * 3:
        raise ValueError(f'expected a {width} x {height} RGB raster')
    raster = bytearray()
    with args.elevation.open(newline='') as source:
        rows = csv.reader(source)
        if next(rows) != ['latitude', 'longitude', 'z']:
            raise ValueError('unexpected raster columns')
        next(rows)  # Unit row returned by ERDDAP.
        for index, row in enumerate(rows):
            latitude, longitude, elevation = map(float, row)
            x, y = index % width, index // width
            if not (math.isclose(longitude, offset + x * step, abs_tol=1e-6)
                    and math.isclose(latitude, -90 + offset + y * step, abs_tol=1e-6)
                    and -12000 <= elevation <= 10000):
                raise ValueError(f'unexpected elevation sample {row}')
            rgb_index = ((height - y - 1) * width + (x + width // 2) % width) * 3
            material = print_class(*rgb[rgb_index:rgb_index + 3])
            raster.extend(struct.pack('<hB', round(elevation), material))
    if len(raster) != width * height * 3:
        raise ValueError('elevation raster is incomplete')
    with ZipFile(args.output, 'w') as archive:
        archive.writestr(ZipInfo('earth.bin'), raster, compress_type=ZIP_DEFLATED, compresslevel=9)


if __name__ == '__main__':
    main()
