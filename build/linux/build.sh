#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the Linux plugins golem installs, from pinned upstream commits (+ our patches), on Ubuntu 24.04 x86_64.
# Each bundle is packed as <name>.tar.gz in OUT, with SHA256SUMS and BUILDINFO.txt.
#
#   build/linux/build.sh OUT [plugin…]      plugins: zconvo nam prisma spectral paulxstretch freeze (default: all)
#
# The pins below must match golem's provision/install-*.sh: the installer falls back to building the same commits
# when a prebuilt bundle cannot be downloaded.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="$(mkdir -p "${1:?usage: build.sh OUT [plugin…]}" && cd "$1" && pwd)"; shift
[ $# -gt 0 ] || set -- zconvo nam prisma spectral paulxstretch freeze
WANT=("$@")
WORK="${WORK:-$ROOT/.work}"; mkdir -p "$WORK"
JOBS=$(nproc)

ZC_REPO=https://github.com/x42/zconvo.lv2.git;                    ZC_COMMIT=c6b88d5ff0ac003c3ce54a5d7f4d6845365b91c8   # v0.7.2
NAM_REPO=https://github.com/mikeoliphant/neural-amp-modeler-lv2.git; NAM_COMMIT=35c28a52ec0a89af16208e953544dc96e13dde20  # v0.2.2
VSTS_REPO=https://github.com/unplugred/vsts.git;                  VSTS_COMMIT=5062b597b01d11507ed60ac9a7e0e7ad6dcfd63f  # Prisma
NIH_REPO=https://github.com/robbert-vdh/nih-plug.git;             NIH_COMMIT=de421011f41a6d10fc8c7a6084e4f4dee0143683   # Spectral Compressor
PXS_REPO=https://github.com/essej/paulxstretch.git;               PXS_COMMIT=b6175e9                                    # v1.6.0

pack() {   # pack <asset-name> <dir holding the bundle> <bundle name>
  tar -C "$2" -czf "$OUT/$1.tar.gz" "$3"
  echo "  ✓ $1.tar.gz"
}

checkout() {   # checkout <repo> <commit> <dir> — submodules retried: gitlab.com refuses "due to load" at times
  local repo="$1" commit="$2" dir="$3" w
  [ -d "$dir/.git" ] || git clone -q "$repo" "$dir"
  git -C "$dir" fetch -q origin
  git -C "$dir" checkout -q -f "$commit"
  git -C "$dir" clean -qfdx
  for w in 0 30 90 180; do
    [ "$w" -gt 0 ] && { echo "  ↻ submodules unavailable, retrying in $w s"; sleep "$w"; }
    git -C "$dir" submodule update -q --init --recursive && return 0
  done
  echo "✗ submodules of $repo could not be fetched" >&2; return 1
}

build_zconvo() {
  echo "== zeroconvo.lv2 (x42 IR convolver) + patches/zconvo-retired-engines.patch"
  local d="$WORK/zconvo.lv2"
  checkout "$ZC_REPO" "$ZC_COMMIT" "$d"
  git -C "$d" apply "$ROOT/patches/zconvo-retired-engines.patch"
  make -C "$d" -j"$JOBS" >/dev/null                       # upstream flags: -O3 -msse -msse2, -DNDEBUG
  grep -q "retired engine" "$d/build/zeroconvolv.so"
  rm -rf "$WORK/stage-zc" && make -C "$d" install LV2DIR="$WORK/stage-zc" >/dev/null
  pack zeroconvo.lv2 "$WORK/stage-zc" zeroconvo.lv2
}

build_nam() {
  echo "== neural_amp_modeler.lv2 + patches/nam-thread-safe-worker.patch"
  local d="$WORK/neural-amp-modeler-lv2"
  checkout "$NAM_REPO" "$NAM_COMMIT" "$d"
  git -C "$d" apply "$ROOT/patches/nam-thread-safe-worker.patch"
  # USE_NATIVE_ARCH=ON means -march=x86-64-v3 (AVX2) in upstream's CMake — like the official release, never -march=native
  cmake -S "$d" -B "$d/build" -DCMAKE_BUILD_TYPE=Release -DUSE_NATIVE_ARCH=ON >/dev/null
  cmake --build "$d/build" -j"$JOBS" >/dev/null
  grep -q "retired model(s) freed by the worker" "$d/build/neural_amp_modeler.lv2/neural_amp_modeler.so"
  pack neural_amp_modeler.lv2 "$d/build" neural_amp_modeler.lv2
}

build_prisma() {
  echo "== Prisma.vst3 (unplugred/vsts, free variant)"
  local d="$WORK/unplugred-vsts"
  checkout "$VSTS_REPO" "$VSTS_COMMIT" "$d"
  cmake -S "$d" -B "$d/build" -DCMAKE_BUILD_TYPE=Release -DBANNERTYPE=0 -DJUCE_COPY_PLUGIN_AFTER_BUILD=OFF >/dev/null
  cmake --build "$d/build" --config Release --target Prisma_VST3 -j"$JOBS" >/dev/null
  local v; v=$(find "$d/build" -type d -name Prisma.vst3 | head -1)
  pack Prisma.vst3 "$(dirname "$v")" Prisma.vst3
}

build_spectral() {
  echo "== Spectral Compressor (nih-plug), without 'standalone'"
  local d="$WORK/nih-plug"
  checkout "$NIH_REPO" "$NIH_COMMIT" "$d"
  sed -i 's/features = \["assert_process_allocs", "standalone"\]/features = ["assert_process_allocs"]/' \
    "$d/plugins/spectral_compressor/Cargo.toml"
  (cd "$d" && cargo xtask bundle spectral_compressor --release >/dev/null)
  pack Spectral-Compressor.clap "$d/target/bundled" "Spectral Compressor.clap"
  pack Spectral-Compressor.vst3 "$d/target/bundled" "Spectral Compressor.vst3"
}

build_paulxstretch() {
  echo "== PaulXStretch (VST3 + CLAP, without JACK or standalone)"
  local d="$WORK/paulxstretch"
  checkout "$PXS_REPO" "$PXS_COMMIT" "$d"
  sed -i 's/JUCE_JACK=1/JUCE_JACK=0/; s/set (FormatsToBuild VST3 CLAP Standalone)/set (FormatsToBuild VST3 CLAP)/' "$d/CMakeLists.txt"
  cmake -S "$d" -B "$d/build" -DCMAKE_BUILD_TYPE=Release -DJUCE_COPY_PLUGIN_AFTER_BUILD=OFF >/dev/null
  cmake --build "$d/build" --config Release -j"$JOBS" >/dev/null
  local a="$d/build/PaulXStretch_artefacts/Release"
  pack PaulXStretch.vst3 "$a/VST3" PaulXStretch.vst3
  pack PaulXStretch.clap "$a/CLAP" PaulXStretch.clap
}

build_freeze() {
  echo "== GoleM Freeze (golem's own block, sources/golem-plugins)"
  local d="$WORK/golem-plugins"
  rm -rf "$d" && cp -r "$ROOT/sources/golem-plugins" "$d"
  (cd "$d" && cargo xtask bundle golem_freeze --release >/dev/null)
  pack GoleM-Freeze.clap "$d/target/bundled" "GoleM Freeze.clap"
  pack GoleM-Freeze.vst3 "$d/target/bundled" "GoleM Freeze.vst3"
}

for p in "${WANT[@]}"; do "build_$p"; done

{
  echo "Built on $(. /etc/os-release && echo "$PRETTY_NAME") $(uname -m), $(date -u +%Y-%m-%dT%H:%MZ)"
  echo "gcc: $(gcc -dumpfullversion)   cmake: $(cmake --version | head -1 | awk '{print $3}')   rustc: $(rustc --version 2>/dev/null | awk '{print $2}')"
  echo "zconvo.lv2 $ZC_COMMIT + patches/zconvo-retired-engines.patch"
  echo "neural-amp-modeler-lv2 $NAM_COMMIT + patches/nam-thread-safe-worker.patch (-march=x86-64-v3)"
  echo "unplugred/vsts $VSTS_COMMIT (Prisma, BANNERTYPE=0)"
  echo "nih-plug $NIH_COMMIT (spectral_compressor, without standalone)"
  echo "paulxstretch $PXS_COMMIT (JUCE_JACK=0, VST3 + CLAP)"
  echo "golem-plugins (golem_freeze) from this repository, $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')"
} > "$OUT/BUILDINFO-$(IFS=-; echo "${WANT[*]}").txt"
(cd "$OUT" && sha256sum ./*.tar.gz | sed 's| \./| |')
