#!/bin/busybox sh
# rk-init, the first version: PID 1 for rk boot, as a busybox shell script.
#
# It mounts what the kernel has, prints the boot marker, runs the suite named by rk.suite on the
# kernel command line and reboots, which ends QEMU because rk boot runs it with -no-reboot. The
# suite is smoke by default. The kunit suite loads every module rk test put under /lib/modules/rk,
# in the order kbuild built them, which runs the KUnit tests they hold, and tries again with the
# ones that failed for as long as that loads more, since a module can need one later in the
# order. Every line rk boot and rk test read starts with RK-, apart from the TAP of KUnit and of the
# selftests, which the kselftest:<collection> suite runs from /kselftest. The ltp:<runtest> suite
# runs one LTP runtest file from /ltp and prints an RK-LTP line per test. The
# C version that docs/plan/11-boot-and-tests.md describes replaces this once rucc can build it,
# and until then both kernels run the same busybox, so the userland is never the difference.

/bin/busybox --install -s /bin
export PATH=/bin
mkdir -p /proc /sys /dev /tmp
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null
mount -t tmpfs tmpfs /tmp 2>/dev/null

suite=smoke
for word in $(cat /proc/cmdline); do
    case $word in
        rk.suite=*) suite=${word#rk.suite=} ;;
    esac
done

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

smoke() {
    check exec /bin/busybox true
    check fork sh -c 'true & wait'
    check pipe sh -c '[ "$(echo rk | cat)" = rk ]'
    check signal sh -c 'sleep 30 & kill $!; wait $!; [ $? -eq 143 ]'
    check tmp-write sh -c 'dd if=/dev/zero of=/tmp/z bs=1024 count=4096 && [ "$(wc -c < /tmp/z)" -eq 4194304 ]'
    check tmp-read sh -c '[ "$(md5sum < /tmp/z)" = "$(dd if=/dev/zero bs=1024 count=4096 2>/dev/null | md5sum)" ]'
    check proc-self test -r /proc/self/status
    check cpus sh -c '[ "$(grep -c ^processor /proc/cpuinfo)" -eq "$(ls -d /sys/devices/system/cpu/cpu[0-9]* | wc -l)" ]'
    check uptime sh -c '[ -n "$(cat /proc/uptime)" ]'
}

kunit() {
    order=/lib/modules/rk/order
    [ -f $order ] || return 0
    left=$(cat $order)
    while [ -n "$left" ]; do
        failed=""
        for module in $left; do
            if insmod /lib/modules/rk/$module 2>/dev/null; then
                echo "RK-MODULE $module pass"
            else
                failed="$failed $module"
            fi
        done
        [ "$(echo $failed)" = "$(echo $left)" ] && break
        left=$failed
    done
    for module in $left; do
        insmod /lib/modules/rk/$module
        echo "RK-MODULE $module fail"
    done
    sleep 1
}

# One collection of the selftests rk test put under /kselftest. run_kselftest.sh prints one TAP
# line per program, and each program's own output behind "# ".
kselftest() {
    [ -x /kselftest/run_kselftest.sh ] || return 0
    cd /kselftest && ./run_kselftest.sh -c "$1"
    cd /
}

# One runtest file of LTP from /ltp. Each test runs in an empty directory under /tmp with its
# output kept aside, which goes to the console behind "# " only when the test fails. LTP exits 32
# when there was nothing to test here, which is a skip.
ltp() {
    file=/ltp/runtest/$1
    [ -f "$file" ] || return 0
    mkdir -p /dev/shm
    mount -t tmpfs tmpfs /dev/shm 2>/dev/null
    export LTPROOT=/ltp TMPDIR=/tmp PATH=/ltp/testcases/bin:/bin
    grep -v '^[[:space:]]*#' "$file" | while read -r tag command; do
        [ -n "$tag" ] && [ -n "$command" ] || continue
        mkdir -p /tmp/rk-ltp
        cd /tmp/rk-ltp
        timeout 600 sh -c "$command" </dev/null >/tmp/rk-ltp.out 2>&1
        code=$?
        cd /
        case $code in
            0) result=pass ;;
            32) result=skip ;;
            *) result=fail ;;
        esac
        [ $result = fail ] && sed 's/^/# /' /tmp/rk-ltp.out
        echo "RK-LTP $1 $tag $result $code"
        rm -rf /tmp/rk-ltp /tmp/rk-ltp.out
    done
    export PATH=/bin
}

case $suite in
    smoke) smoke ;;
    kunit) kunit ;;
    kselftest:*) kselftest "${suite#kselftest:}" ;;
    ltp:*) ltp "${suite#ltp:}" ;;
    *) echo "RK-SUITE $suite unknown" ;;
esac

echo "RK-DONE"
sync
reboot -f
