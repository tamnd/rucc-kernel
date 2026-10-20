# rk-era-bullseye: the reference toolchain for the eras in personas.toml that name it.
#
# GCC 10 from the distribution is linked as gcc, cc and cpp, so the kernel's plain gcc is the
# reference. The GCC plugin headers are left out on purpose, which keeps GCC_PLUGINS off on both
# sides (plan 5.4). rk personas check reads the versions back and fails if they drift.

FROM debian/eol:bullseye

ARG DEBIAN_FRONTEND=noninteractive
ARG TARGETARCH

# Bullseye is in the archive now. The base image has the LTS updates of libc6, libssl1.1 and
# perl-base, so the image takes the security pool from the archive too, or the -dev packages of the
# last point release do not install on top of them.
RUN printf '%s\n' "deb http://archive.debian.org/debian bullseye main" \
    "deb http://archive.debian.org/debian-security bullseye-security main" > /etc/apt/sources.list \
 && rm -f /etc/apt/sources.list.d/*

RUN apt-get -o Acquire::Check-Valid-Until=false update \
 && apt-get install -y --no-install-recommends -o APT::Get::AllowUnauthenticated=true \
    gcc-10 cpp-10 make perl bc bison flex binutils libc6-dev libelf-dev libssl-dev kmod cpio xz-utils bzip2 patch python3 rsync ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# The 32-bit multilib is for the X32 row and the x86-64 vDSO, and only exists on amd64.
RUN if [ "$TARGETARCH" = amd64 ]; then apt-get update \
 && apt-get install -y --no-install-recommends gcc-10-multilib \
 && rm -rf /var/lib/apt/lists/*; fi

RUN ln -sf /usr/bin/gcc-10 /usr/local/bin/gcc \
 && ln -sf /usr/bin/gcc-10 /usr/local/bin/cc \
 && ln -sf /usr/bin/cpp-10 /usr/local/bin/cpp \
 && gcc --version | head -n 1

WORKDIR /work
