# rk-era-lenny: the reference toolchain for the eras in personas.toml that name it.
#
# GCC 4.3 from the distribution is linked as gcc, cc and cpp, so the kernel's plain gcc is the
# reference. The GCC plugin headers are left out on purpose, which keeps GCC_PLUGINS off on both
# sides (plan 5.4). rk personas check reads the versions back and fails if they drift.

FROM debian/eol:lenny

ARG DEBIAN_FRONTEND=noninteractive
ARG TARGETARCH

RUN apt-get -o Acquire::Check-Valid-Until=false update \
 && apt-get install -y --no-install-recommends -o APT::Get::AllowUnauthenticated=true \
    gcc-4.3 cpp-4.3 make perl bc binutils libc6-dev cpio bzip2 patch libc6-dev-i386 module-init-tools \
 && rm -rf /var/lib/apt/lists/*

RUN ln -sf /usr/bin/gcc-4.3 /usr/local/bin/gcc \
 && ln -sf /usr/bin/gcc-4.3 /usr/local/bin/cc \
 && ln -sf /usr/bin/cpp-4.3 /usr/local/bin/cpp \
 && gcc --version | head -n 1

WORKDIR /work
