#!/bin/sh
# One step of the programs suite, run inside the guest as root by run.sh. A step prints a line
# `RK-RESULT key value` for each thing the host keeps, and its exit status is the step's own.
# compare.py reads the keys: a key it lists as a set is compared as a set of values, a rate as
# a number where higher is better, and any other key is shown as it is.

set -u
step=$1

result() { echo "RK-RESULT $*"; }

# The git release that is built and tested, from kernel.org's signed sha256sums.asc.
GIT_VERSION=2.56.1
GIT_SHA256=f0b0bf89264b9bd2e1c640363e7529643989da76281600473c50745cd34a607f

# Every package a step uses. prepare installs them once, on the gcc kernel, and both kernels
# then boot copies of that disk. Git 2.56 builds a part of itself in Rust, hence cargo.
PACKAGES="gcc make tar xz zlib-devel openssl-devel libcurl-devel expat-devel perl
perl-Test-Harness diffutils nginx httpd-tools postgresql-server postgresql-contrib stress-ng
podman e2fsprogs btrfs-progs xfsprogs fio cloud-utils-growpart cargo perf"

# The container images, none from Docker Hub, whose pull limit hosted runners share.
ALPINE=quay.io/libpod/alpine:latest
WEB=quay.io/nginx/nginx-unprivileged:alpine
FEDORA=registry.fedoraproject.org/fedora:45
IMAGES="$ALPINE $WEB $FEDORA"

case $step in
prepare)
    # The image is a few GB, so the root takes the rest of the disk run.sh grew.
    source=$(findmnt -no SOURCE / | sed 's/\[.*//')
    disk=/dev/$(lsblk -no PKNAME "$source")
    number=$(echo "$source" | sed 's/.*[^0-9]//')
    command -v growpart > /dev/null || dnf -y -q install cloud-utils-growpart
    growpart "$disk" "$number"
    case $(findmnt -no FSTYPE /) in
    btrfs) btrfs filesystem resize max / ;;
    xfs) xfs_growfs / ;;
    ext4) resize2fs "$source" ;;
    esac
    df -h /
    # shellcheck disable=SC2086
    dnf -y -q install --setopt=install_weak_deps=False $PACKAGES || exit 1
    useradd -m rk
    curl -fsSLo /home/rk/git.tar.xz \
        "https://mirrors.edge.kernel.org/pub/software/scm/git/git-$GIT_VERSION.tar.xz"
    echo "$GIT_SHA256  /home/rk/git.tar.xz" | sha256sum -c - || exit 1
    chown rk:rk /home/rk/git.tar.xz
    for image in $IMAGES; do
        podman pull -q "$image" > /dev/null || exit 1
    done
    systemctl disable --now firewalld 2> /dev/null
    sync
    ;;

systemd)
    result kernel "$(uname -r)"
    result state "$(systemctl is-system-running --wait)"
    systemctl --failed --plain --no-legend | while read -r unit _; do
        result failed-unit "$unit"
    done
    result getty "$(systemctl is-active serial-getty@ttyS0.service)"
    # A login through PAM, as a user, which logind sees as a session of its own.
    runuser -l rk -c 'id -un' | grep -qx rk || exit 1
    result sessions "$(loginctl list-sessions --no-legend | wc -l)"
    result modules "$(wc -l < /proc/modules)"
    ;;

git)
    # Build git and run its own test suite as a user. Make keeps going past a failing script,
    # and aggregate-results says which ones failed. The scripts are named one by one, since the
    # test target cleans the results away when every script passes.
    runuser -l rk -c "
        set -e
        rm -rf git-$GIT_VERSION
        tar -xf git.tar.xz
        cd git-$GIT_VERSION
        make -s -j\$(nproc) NO_GETTEXT=1 NO_TCLTK=1 NO_PYTHON=1 > ../git-build.log 2>&1
        cd t
        make -k -j\$(nproc) NO_GETTEXT=1 NO_TCLTK=1 NO_PYTHON=1 t[0-9]*.sh \
            > ../../git-test.log 2>&1 || true
        make aggregate-results > ../../git-results.log 2>&1 || true
    " || { tail -n 40 /home/rk/git-build.log; exit 1; }
    cat /home/rk/git-results.log
    for count in fixed success failed broken total; do
        result "git-$count" "$(awk -v k="$count" '$1 == k { print $2 }' /home/rk/git-results.log)"
    done
    sed -n 's/^failed test(s): //p' /home/rk/git-results.log | tr ' ' '\n' | while read -r t; do
        [ -n "$t" ] && result git-failed "$t"
    done
    grep -q '^total' /home/rk/git-results.log
    ;;

nginx)
    # The files the host fetches over the virtio network. The big one's digest is the check
    # that it came through whole.
    mkdir -p /usr/share/nginx/html/rk
    echo small > /usr/share/nginx/html/rk/small.html
    head -c 4194304 /dev/urandom > /usr/share/nginx/html/rk/big
    result big-sha256 "$(sha256sum < /usr/share/nginx/html/rk/big | cut -d' ' -f1)"
    systemctl enable --now nginx || exit 1
    curl -fsS http://127.0.0.1/rk/small.html | grep -qx small
    ;;

pgbench)
    postgresql-setup --initdb > /dev/null || exit 1
    systemctl start postgresql || exit 1
    runuser -u postgres -- pgbench -i -q -s 20 postgres > /dev/null 2>&1 || exit 1
    bad=0
    for _ in 1 2 3; do
        runuser -u postgres -- pgbench -n -c 8 -j 4 -T 30 postgres > /tmp/pgbench.out 2>&1
        status=$?
        cat /tmp/pgbench.out
        tps=$(sed -n 's/^tps = \([0-9.]*\).*/\1/p' /tmp/pgbench.out)
        failed=$(sed -n 's/^number of failed transactions: \([0-9]*\).*/\1/p' /tmp/pgbench.out)
        result pgbench-step-tps "${tps:-0}"
        [ "$status" -eq 0 ] && [ "${failed:-1}" = 0 ] || bad=1
    done
    exit $bad
    ;;

fs)
    # Each file system on the scratch disk: a verified fio run, a copy of /usr/lib64 read back
    # after a remount, and the file system's own check once it is unmounted. fio uses psync,
    # which is built in, as Fedora ships libaio as a package of its own. The copy has symlinks
    # that point nowhere, so diff compares the links and does not follow them.
    scratch=/dev/vdb
    mkdir -p /mnt/rk
    bad=0
    for fs in ext4 btrfs xfs; do
        case $fs in
        ext4) mkfs.ext4 -q -F "$scratch" ;;
        btrfs) mkfs.btrfs -q -f "$scratch" ;;
        xfs) mkfs.xfs -q -f "$scratch" ;;
        esac
        if ! mount -t "$fs" "$scratch" /mnt/rk; then
            result fs-failed "$fs-mount"
            bad=1
            continue
        fi
        fio --name=verify --directory=/mnt/rk --size=256M --numjobs=2 --rw=randwrite --bs=4k \
            --ioengine=psync --direct=1 --verify=crc32c --group_reporting \
            > "/tmp/fio-$fs.log" 2>&1 \
            || { result fs-failed "$fs-fio"; tail -n 30 "/tmp/fio-$fs.log"; bad=1; }
        cp -a /usr/lib64 /mnt/rk/lib64 || { result fs-failed "$fs-copy"; bad=1; }
        umount /mnt/rk
        mount -t "$fs" "$scratch" /mnt/rk
        diff -r --no-dereference /usr/lib64 /mnt/rk/lib64 > "/tmp/diff-$fs.log" 2>&1 \
            || { result fs-failed "$fs-diff"; head -n 30 "/tmp/diff-$fs.log"; bad=1; }
        umount /mnt/rk
        case $fs in
        ext4) e2fsck -fn "$scratch" ;;
        btrfs) btrfs check --readonly "$scratch" ;;
        xfs) xfs_repair -n "$scratch" ;;
        esac > "/tmp/check-$fs.log" 2>&1 \
            || { result fs-failed "$fs-check"; tail -n 30 "/tmp/check-$fs.log"; bad=1; }
        result fs-done "$fs"
    done
    exit $bad
    ;;

podman)
    [ "$(stat -fc %T /sys/fs/cgroup)" = cgroup2fs ] || { echo "not cgroup v2"; exit 1; }
    bad=0
    limits=$(podman run --rm --memory 64m --pids-limit 64 "$ALPINE" \
        cat /sys/fs/cgroup/memory.max /sys/fs/cgroup/pids.max | tr '\n' ' ')
    result podman-limits "$limits"
    [ "$limits" = "67108864 64 " ] || bad=1
    # The shell keeps the whole output in memory, which is more than the limit, so it is killed.
    podman run --rm --memory 64m "$ALPINE" \
        sh -c 'x=$(head -c 268435456 /dev/zero | tr "\\0" a); echo ${#x}'
    oom=$?
    result podman-oom "$oom"
    [ "$oom" = 137 ] || bad=1
    # Two containers on a network of their own, one fetching from the other by name.
    podman network create rk > /dev/null
    podman run -d --name web --network rk "$WEB" > /dev/null || bad=1
    sleep 3
    podman run --rm --network rk "$ALPINE" wget -qO- http://web:8080/ \
        | grep -q nginx || { result podman-failed network; bad=1; }
    podman rm -f web > /dev/null
    # Part of CPython's test suite in a Fedora container, the parts that lean on the kernel. A
    # failed test runs once more, and only one that fails again is a failure. test_socket and
    # test_posix each failed on one kernel or the other on some nights and passed on others.
    podman run --rm --memory 2g "$FEDORA" sh -c '
        dnf -y -q install python3-test > /dev/null &&
        python3 -m test -j4 -w --timeout 900 test_os test_posix test_socket test_subprocess \
            test_signal test_threading test_select test_selectors test_mmap test_fcntl \
            test_epoll test_poll test_pty test_asyncio test_shutil test_tempfile' \
        > /tmp/python.log 2>&1
    tail -n 30 /tmp/python.log
    result python "$(sed -n 's/^Result: //p' /tmp/python.log | tail -n 1)"
    sed -n '/tests\{0,1\} failed:/,/^$/p' /tmp/python.log | tr -s ' ' '\n' \
        | grep '^test_' | while read -r t; do result python-failed "$t"; done
    grep -q '^Result: ' /tmp/python.log || bad=1
    exit $bad
    ;;

rates)
    # The rates that are compared, measured inside the guest so that the virtio network and the
    # host's load stay out of them: ab on nginx over loopback and pgbench, three rounds each.
    # The workflow boots each kernel a few times, one after the other, and runs this step alone
    # on each boot, so the step sets up what it needs whatever ran before it.
    mkdir -p /usr/share/nginx/html/rk
    echo small > /usr/share/nginx/html/rk/small.html
    systemctl start nginx || exit 1
    [ -f /var/lib/pgsql/data/PG_VERSION ] || postgresql-setup --initdb > /dev/null || exit 1
    systemctl start postgresql || exit 1
    runuser -u postgres -- pgbench -i -q -s 20 postgres > /dev/null 2>&1 || exit 1
    bad=0
    for _ in 1 2 3; do
        ab -q -n 50000 -c 32 http://127.0.0.1/rk/small.html > /tmp/ab.out 2>&1
        result nginx-rps "$(awk '/^Requests per second/ { print $4 }' /tmp/ab.out)"
        awk '/^Failed requests/ { f = $3 } /^Non-2xx responses/ { n = $3 }
            END { exit f + n > 0 }' /tmp/ab.out || bad=1
        runuser -u postgres -- pgbench -n -c 8 -j 4 -T 20 postgres > /tmp/pgbench.out 2>&1 || bad=1
        result pgbench-tps "$(sed -n 's/^tps = \([0-9.]*\).*/\1/p' /tmp/pgbench.out)"
    done
    [ "$bad" = 0 ] || { cat /tmp/ab.out /tmp/pgbench.out; exit 1; }
    ;;

profile)
    # Where the kernel spends the time of the rates' ab load, so that a rate one kernel loses can
    # be put down to the functions that cost it. perf samples every CPU on the clock, since a guest
    # has no counters. The number of requests is fixed, so a symbol's count of samples is its CPU
    # time for the same work, and the counts of two kernels can be compared as they are. The report
    # gives the counts by process and by kernel symbol, and by kernel symbol for ab and nginx alone,
    # since ab is one thread and its CPU is the one the rate waits on.
    sysctl -qw kernel.kptr_restrict=0 kernel.perf_event_paranoid=-1
    mkdir -p /usr/share/nginx/html/rk
    echo small > /usr/share/nginx/html/rk/small.html
    systemctl start nginx || exit 1
    ab -q -n 20000 -c 32 http://127.0.0.1/rk/small.html > /dev/null 2>&1
    perf record -q -a -g -e cpu-clock -F 4999 -o /tmp/perf.data -- \
        ab -q -n 200000 -c 32 http://127.0.0.1/rk/small.html > /tmp/ab.out 2>&1 || exit 1
    result nginx-rps "$(awk '/^Requests per second/ { print $4 }' /tmp/ab.out)"
    report() { perf report -i /tmp/perf.data --stdio -q -n --no-children -g none "$@" 2> /dev/null; }
    echo "by process"
    report --sort comm | head -12
    echo "by kernel symbol"
    report --sort sym | grep -F '[k]' | head -80
    for comm in ab nginx; do
        echo "by kernel symbol in $comm"
        report --comms "$comm" --sort sym | grep -F '[k]' | head -40
    done
    # Who calls the library's copy and fill, folded one chain to a line, since a call gcc writes
    # inline costs the rucc kernel a call and a return thunk each time.
    echo "callers of the copy and fill routines"
    perf report -i /tmp/perf.data --stdio -q --no-children --sort sym --max-stack 4 \
        --symbols memset_orig,__memset,memcpy_orig,__memcpy,__memmove,memmove \
        -g folded,0.5,callee,function,count 2> /dev/null | head -80
    ;;

stress)
    # An hour of stress-ng: four stressors at a time, each of the classes in turn, 15 seconds
    # each. A stressor that fails is listed, and the dmesg check in compare.py catches the rest.
    mkdir -p /var/tmp/rk
    start=$(date +%s)
    stress-ng --sequential 4 --timeout 15s --class cpu,memory,filesystem,scheduler,network \
        --temp-path /var/tmp/rk --metrics-brief --oomable > /tmp/stress.log 2>&1
    status=$?
    tail -n 40 /tmp/stress.log
    result stress-exit "$status"
    result stress-seconds "$(($(date +%s) - start))"
    sed -n 's/.*failed: [0-9]*: \(.*\)$/\1/p' /tmp/stress.log | tr ' ' '\n' \
        | sed 's/(.*//' | grep . | while read -r s; do result stress-failed "$s"; done
    ;;

*)
    echo "no step $step"
    exit 2
    ;;
esac
