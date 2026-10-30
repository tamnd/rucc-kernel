# rk-era-forky: the second reference of E11 in personas.toml, GCC 16 from Debian forky.
#
# GCC 16 is linked as gcc, cc and cpp, so the kernel's plain gcc is the second reference. The GCC plugin headers are left out on purpose, which keeps GCC_PLUGINS off on both sides (plan 5.4). rk personas check reads the versions back and fails if they drift.
#
# gawk is in the list because scripts/Makefile.vmlinux runs generate_builtin_ranges.awk with gawk when CONFIG_BUILTIN_MODULE_RANGES is set, and allmodconfig sets it. mawk is not enough.
#
# zstd is in the list because the Fedora x86-64 config sets CONFIG_KERNEL_ZSTD, and the bzImage build then runs zstd on vmlinux.

FROM debian:forky

ARG DEBIAN_FRONTEND=noninteractive
ARG TARGETARCH

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
    gcc-16 cpp-16 make perl bc bison flex binutils libc6-dev libelf-dev libssl-dev kmod cpio gawk xz-utils zstd bzip2 patch python3 rsync ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# The 32-bit multilib is for the X32 row and the x86-64 vDSO, and only exists on amd64.
RUN if [ "$TARGETARCH" = amd64 ]; then apt-get update \
 && apt-get install -y --no-install-recommends gcc-16-multilib \
 && rm -rf /var/lib/apt/lists/*; fi

RUN ln -sf /usr/bin/gcc-16 /usr/local/bin/gcc \
 && ln -sf /usr/bin/gcc-16 /usr/local/bin/cc \
 && ln -sf /usr/bin/cpp-16 /usr/local/bin/cpp \
 && gcc --version | head -n 1

WORKDIR /work
