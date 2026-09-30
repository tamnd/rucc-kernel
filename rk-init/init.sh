#!/bin/busybox sh
# rk-init, the first version: PID 1 for rk boot, as a busybox shell script.
#
# It mounts what the kernel has, prints the boot marker, runs the smoke checks and reboots, which
# ends QEMU because rk boot runs it with -no-reboot. Every line rk boot reads starts with RK-. The
# C version that docs/plan/11-boot-and-tests.md describes replaces this once rucc can build it,
# and until then both kernels run the same busybox, so the userland is never the difference.

/bin/busybox --install -s /bin
export PATH=/bin
mkdir -p /proc /sys /dev /tmp
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null
mount -t tmpfs tmpfs /tmp 2>/dev/null

echo "RK-BOOTED $(uname -r) $(cut -d' ' -f1 /proc/uptime)"

check() {
    name=$1
    shift
    if "$@" >/dev/null 2>&1; then
        echo "RK-CHECK $name pass"
    else
        echo "RK-CHECK $name fail"
    fi
}

check exec /bin/busybox true
check fork sh -c 'true & wait'
check pipe sh -c '[ "$(echo rk | cat)" = rk ]'
check signal sh -c 'sleep 30 & kill $!; wait $!; [ $? -eq 143 ]'
check tmp-write sh -c 'dd if=/dev/zero of=/tmp/z bs=1024 count=4096 && [ "$(wc -c < /tmp/z)" -eq 4194304 ]'
check tmp-read sh -c '[ "$(md5sum < /tmp/z)" = "$(dd if=/dev/zero bs=1024 count=4096 2>/dev/null | md5sum)" ]'
check proc-self test -r /proc/self/status
check cpus sh -c '[ "$(grep -c ^processor /proc/cpuinfo)" -eq "$(ls -d /sys/devices/system/cpu/cpu[0-9]* | wc -l)" ]'
check uptime sh -c '[ -n "$(cat /proc/uptime)" ]'

echo "RK-DONE"
sync
reboot -f
