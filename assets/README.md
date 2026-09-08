# Application icon

`icon.svg` is the original project artwork: an open book with a command prompt.
It is licensed under the repository's AGPL-3.0-only license.
`icon.png` supplies the running window icon; `icon.ico` is embedded in Windows
executables by `build.rs`, including ordinary local Cargo builds.

Regenerate the committed assets with ImageMagick (not required to build):

```sh
magick -background none assets/icon.svg -resize 256x256 -depth 8 PNG32:assets/icon.png
magick assets/icon.png -define icon:auto-resize=256,128,64,48,32,24,16 assets/icon.ico
```

Windows builds need a resource compiler: the Windows SDK for MSVC, or MinGW
`windres` for GNU targets. Release CI verifies that Windows can extract both
large and small icons from the final executable before packaging it.
