#!/usr/bin/env python3
"""Pack the reproducible half-degree Whole Earth raster with its print classes."""

import argparse
import csv
import math
import struct
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

WIDTH, HEIGHT = 720, 360


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
    parser.add_argument('land_cover', type=Path, help='720 x 360 RGB bytes in north-to-south order')
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    rgb = args.land_cover.read_bytes()
    if len(rgb) != WIDTH * HEIGHT * 3:
        raise ValueError('expected a 720 x 360 RGB raster')
    raster = bytearray()
    with args.elevation.open(newline='') as source:
        rows = csv.reader(source)
        if next(rows) != ['latitude', 'longitude', 'z']:
            raise ValueError('unexpected raster columns')
        next(rows)  # Unit row returned by ERDDAP.
        for index, row in enumerate(rows):
            latitude, longitude, elevation = map(float, row)
            x, y = index % WIDTH, index // WIDTH
            if not (math.isclose(longitude, 0.2416666667 + x / 2, abs_tol=1e-6)
                    and math.isclose(latitude, -89.7583333333 + y / 2, abs_tol=1e-6)
                    and -12000 <= elevation <= 10000):
                raise ValueError(f'unexpected elevation sample {row}')
            rgb_index = ((HEIGHT - y - 1) * WIDTH + (x + WIDTH // 2) % WIDTH) * 3
            material = print_class(*rgb[rgb_index:rgb_index + 3])
            raster.extend(struct.pack('<hB', round(elevation), material))
    if len(raster) != WIDTH * HEIGHT * 3:
        raise ValueError('elevation raster is incomplete')
    with ZipFile(args.output, 'w') as archive:
        archive.writestr(ZipInfo('earth.bin'), raster, compress_type=ZIP_DEFLATED, compresslevel=9)


if __name__ == '__main__':
    main()
