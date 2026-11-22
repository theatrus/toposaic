# Whole Earth globe data

`earth-half-degree.zip` contains a 720 × 360 grid. Each cell has a signed
little-endian 16-bit elevation in meters and a byte from 0 to 4 identifying its
print color class. Longitude increases east from 0.2416666667° and latitude
increases north from −89.7583333333°, both in 0.5° steps.

Elevation comes from NOAA NCEI ETOPO 2022, 60 arc-second ice surface, sampled at
half-degree intervals. The land and water categories are derived from the colors
of the public-domain Natural Earth I with Shaded Relief and Water raster at
1:50 million. They are simplified printing categories. Map shading and smooth
transitions can affect their classification.

- [NOAA ETOPO 2022](https://www.ncei.noaa.gov/products/etopo-global-relief-model)
- [Natural Earth I](https://www.naturalearthdata.com/downloads/50m-raster-data/50m-natural-earth-1/)

Rebuild the bundled grid with Python 3 and ImageMagick.

```sh
curl -fL 'https://oceanwatch.pifsc.noaa.gov/erddap/griddap/ETOPO_2022_v1_60s.csv?z%5B14:30:10799%5D%5B14:30:21599%5D' -o etopo.csv
curl -fL 'https://naturalearth.s3.amazonaws.com/50m_raster/NE1_50M_SR_W.zip' -o natural-earth.zip
unzip -p natural-earth.zip NE1_50M_SR_W/NE1_50M_SR_W.tif > natural-earth.tif
magick natural-earth.tif -colorspace sRGB -resize '720x360!' -depth 8 rgb:land-cover.rgb
python3 scripts/build-globe-earth-data.py etopo.csv land-cover.rgb assets/globe/earth-half-degree.zip
```
