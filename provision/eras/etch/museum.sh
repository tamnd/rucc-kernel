#!/bin/bash
# The from-source toolchains of eras E0 to E2 (plan 4.4), for the museum kernels of K11.
#
# This is the start of the recipe and not yet part of the image. It is meant to run in an i386
# etch container, docker run --platform linux/386 debian/eol:etch, since the kernels it serves
# are i386 only and GCC 2.x does not know x86-64. Each GCC is installed under /opt with the
# binutils of its era, so /opt/gcc-2.95.3/bin/gcc finds its own as and ld. Every tarball is
# checked against the SHA-256 below before it is unpacked.
#
# Known work before this goes into the Dockerfile: GCC 2.7.2.3 and 2.95.3 need the etch GCC 3.4
# as the bootstrap compiler, since GCC 4.1 rejects parts of their sources, and binutils 2.9.1
# predates the configure fixes for a modern host triplet.

set -euo pipefail

mirror=${GNU_MIRROR:-https://ftp.gnu.org/gnu}
work=${WORK:-/tmp/museum}
jobs=$(nproc)

# name, path under the mirror, SHA-256
tarballs=(
  "binutils-2.9.1 binutils/binutils-2.9.1.tar.gz 58d01daa576d8779e064922171276795f00ff1388b1b0f87aa1a00eb0da6c6bb"
  "binutils-2.12.1 binutils/binutils-2.12.1.tar.bz2 05bb06c02b197066986d91b53269d12d2580bc52956575fc203eb52a57bcd335"
  "binutils-2.16.1 binutils/binutils-2.16.1.tar.bz2 d78a6ff982ab2f73721706e7bfd3285ac647804664e7ea4786e66d01f91c56c5"
  "gcc-2.7.2.3 gcc/gcc-2.7.2.3.tar.gz 16166e0f0f2064bb3114716650569065ef3855ae2331d7a29b85d4b9aa73773c"
  "gcc-2.95.3 gcc/gcc-2.95.3/gcc-everything-2.95.3.tar.gz 2a950e220c2f64c4abf781be3bb6d4c472ef8b3685873e77061788df85c6d5da"
)

# gcc, binutils it pairs with, the eras it serves
toolchains=(
  "gcc-2.7.2.3 binutils-2.9.1 E0,E1"
  "gcc-2.95.3 binutils-2.12.1 E2"
)

fetch() {
  local name=$1 path=$2 sum=$3 file
  file="$work/$(basename "$path")"
  [ -f "$file" ] || wget -q -O "$file" "$mirror/$path"
  echo "$sum  $file" | sha256sum -c - >/dev/null
  mkdir -p "$work/src"
  tar -xf "$file" -C "$work/src"
  echo "unpacked $name"
}

build_binutils() {
  local name=$1 prefix=$2
  mkdir -p "$work/build/$name-$(basename "$prefix")"
  cd "$work/build/$name-$(basename "$prefix")"
  "$work/src/$name/configure" --prefix="$prefix" --host=i686-pc-linux-gnu --disable-nls
  make -j"$jobs"
  make install
}

build_gcc() {
  local name=$1 prefix=$2
  mkdir -p "$work/build/$name"
  cd "$work/build/$name"
  CC=gcc-3.4 PATH="$prefix/bin:$PATH" "$work/src/$name/configure" --prefix="$prefix" \
    --host=i686-pc-linux-gnu --enable-languages=c --disable-nls
  PATH="$prefix/bin:$PATH" make -j"$jobs" CC=gcc-3.4 LANGUAGES=c
  PATH="$prefix/bin:$PATH" make install LANGUAGES=c
}

mkdir -p "$work"
for t in "${tarballs[@]}"; do
  read -r name path sum <<<"$t"
  fetch "$name" "$path" "$sum"
done
for t in "${toolchains[@]}"; do
  read -r gcc binutils eras <<<"$t"
  prefix="/opt/$gcc"
  echo "building $gcc with $binutils into $prefix for $eras"
  build_binutils "$binutils" "$prefix"
  build_gcc "$gcc" "$prefix"
done

# E3's GCC 3.4.6 is etch's own package, and only the binutils of its era come from source.
build_binutils binutils-2.16.1 /opt/binutils-2.16.1
