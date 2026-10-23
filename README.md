# golem-binaries

Prebuilt **third-party plugins** that the [golem](https://golem.fm) installer downloads, so that a new install
does not have to compile them, or depend on a package source that may disappear. Nothing here is golem itself,
except the small Freeze block (`sources/golem-plugins`).

Every file is pinned by its SHA-256 in golem's `provision/downloads.sha256` (Linux) or `provision/*/pins.json`
(Windows, macOS): the installer keeps a download only if its hash matches. If a download fails, the Linux installer
builds the same pinned commits itself.

## Releases

| Release | What | Built from | License |
|---|---|---|---|
| `linux-plugins-v*` | `zeroconvo.lv2` — x42 IR convolver, with [our patch](patches/zconvo-retired-engines.patch) (crash on rapid preset changes) | [x42/zconvo.lv2](https://github.com/x42/zconvo.lv2) `c6b88d5` (v0.7.2) | GPL-2.0-or-later |
| | `neural_amp_modeler.lv2` — NAM, with [our patch](patches/nam-thread-safe-worker.patch) (crash on rapid patch changes), `-march=x86-64-v3` | [mikeoliphant/neural-amp-modeler-lv2](https://github.com/mikeoliphant/neural-amp-modeler-lv2) `35c28a5` (v0.2.2) | GPL-3.0 |
| | `Prisma.vst3` — multiband distortion (free variant) | [unplugred/vsts](https://github.com/unplugred/vsts) `5062b59` | AGPL-3.0 |
| | `Spectral Compressor` (VST3, CLAP), without its standalone | [robbert-vdh/nih-plug](https://github.com/robbert-vdh/nih-plug) `de42101` | GPL-3.0-or-later |
| | `PaulXStretch` (VST3, CLAP), without JACK or standalone | [essej/paulxstretch](https://github.com/essej/paulxstretch) `b6175e9` (v1.6.0) | GPL-3.0 |
| | `GoleM Freeze` (VST3, CLAP) — golem's spectral freeze block | [`sources/golem-plugins`](sources/golem-plugins) | GPL-3.0-or-later |
| `nam-0.7.15-golem` | NeuralAmpModeler 0.7.15 for Windows and macOS (VST3, AU) — upstream publishes no binary for 0.7.14/0.7.15 | [Goldoralf/NeuralAmpModelerPlugin](https://github.com/Goldoralf/NeuralAmpModelerPlugin), tag `v0.7.15`, unchanged source | MIT (+ Eigen MPL-2, iPlug2) |
| `sfizz-1.2.3-golem` | sfizz 1.2.3 Ubuntu 24.04 packages, byte for byte — its openSUSE repository was emptied in 2026-10 | [sfztools/sfizz](https://github.com/sfztools/sfizz), packages built by sfztools on the openSUSE Build Service | BSD-2-Clause |

## Source for the binaries

The Linux plugins are built by [`build/linux/build.sh`](build/linux/build.sh) in the
[`linux-plugins`](.github/workflows/linux-plugins.yml) workflow on GitHub's Ubuntu 24.04 runners: pinned upstream
commits, the patches in [`patches/`](patches), and the build options written in the script. Each release carries
`BUILDINFO.txt` (commits, compiler versions) and `SHA256SUMS`. To rebuild one yourself on Ubuntu 24.04:

```sh
build/linux/build.sh out nam        # or: zconvo prisma spectral paulxstretch freeze — none = all
```

The patches, the build script and `sources/` are GPL-3.0-or-later ([LICENSE](LICENSE)); each binary keeps its own
license, above.
