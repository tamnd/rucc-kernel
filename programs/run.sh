#!/bin/bash
# Boot one kernel with a Fedora Cloud userspace under KVM and run program steps in it.
#
#   run.sh BZIMAGE MODULES_DIR DISK OUT STEP...
#
# MODULES_DIR is the lib/modules/RELEASE directory rk modules-install printed. DISK is a qcow2
# the guest may write to, normally an overlay on the shared image. Each step runs in the guest
# under systemd-run, so that it does not depend on the ssh connection staying up, and the host
# asks every 30 seconds whether it has finished. OUT gets the console, each step's log, the
# dmesg, and results.tsv with a line `step key value` for each result and each step's status.
#
# A step can have a part on the host: after nginx, ab loads the guest's nginx through the
# forwarded port, which crosses the virtio network. Those rates are kept to look at, while the
# ones compare.py holds the kernels to come from the rates step inside the guest.

set -u
here=$(cd "$(dirname "$0")" && pwd)
kernel=$1 modules=$2 disk=$3 out=$4
shift 4
mkdir -p "$out"
out=$(cd "$out" && pwd)
release=$(basename "$modules")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# How long each step may take, in seconds, before it counts as a hang.
limit() {
    case $1 in
    prepare) echo 1800 ;;
    git) echo 3600 ;;
    stress) echo 5400 ;;
    podman) echo 2400 ;;
    *) echo 900 ;;
    esac
}

ssh-keygen -q -t ed25519 -N '' -f "$work/key"
initramfs=$work/initramfs
mkdir -p "$initramfs"/{bin,proc,sys,dev,sysroot}
cp "$(command -v busybox)" "$initramfs/bin/busybox"
cp "$here/init" "$initramfs/init"
cp "$here/rk.nmconnection" "$initramfs/rk.nmconnection"
cp "$work/key.pub" "$initramfs/authorized_keys"
echo "$release" > "$initramfs/release"
tar -cf "$initramfs/modules.tar" -C "$(dirname "$modules")" "$release"
chmod 755 "$initramfs/init"
(cd "$initramfs" && find . | cpio -o -H newc --quiet) > "$work/initramfs.cpio"
rm -rf "$initramfs"

qemu-img create -q -f qcow2 "$work/scratch.qcow2" 16G
qemu-system-x86_64 -enable-kvm -cpu host -smp 4 -m 8G -display none -no-reboot \
    -kernel "$kernel" -initrd "$work/initramfs.cpio" \
    -append "console=ttyS0 enforcing=0 panic=-1 systemd.show_status=1" \
    -drive "file=$disk,if=virtio,format=qcow2" \
    -drive "file=$work/scratch.qcow2,if=virtio,format=qcow2" \
    -netdev user,id=net0,hostfwd=tcp:127.0.0.1:2222-:22,hostfwd=tcp:127.0.0.1:8080-:80 \
    -device virtio-net-pci,netdev=net0 \
    -serial "file:$out/console.log" -monitor none &
qemu=$!

ssh_opts=(-i "$work/key" -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
    -o BatchMode=yes -o LogLevel=ERROR -o ConnectTimeout=10 -o ServerAliveInterval=15
    -o ServerAliveCountMax=4)
guest() { ssh "${ssh_opts[@]}" root@127.0.0.1 "$@"; }
record() { printf '%s\t%s\t%s\n' "$@" >> "$out/results.tsv"; }

: > "$out/results.tsv"
up=0
for _ in $(seq 120); do
    if ! kill -0 "$qemu" 2> /dev/null; then
        break
    fi
    if guest true 2> /dev/null; then
        up=1
        break
    fi
    sleep 5
done
if [ "$up" = 0 ]; then
    record boot status no-ssh
    echo "the guest never answered on ssh, the console ends with:"
    tail -n 60 "$out/console.log"
    kill "$qemu" 2> /dev/null
    wait "$qemu"
    exit 1
fi
record boot status 0
scp -q -P 2222 -i "$work/key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o LogLevel=ERROR "$here/steps.sh" root@127.0.0.1:/root/rk-steps.sh

failed=0
for step in "$@"; do
    echo "::group::$step"
    guest "rm -f /root/rk-$step.status; systemd-run -q --unit=rk-$step --setenv=HOME=/root \
        -p WorkingDirectory=/root sh -c 'sh /root/rk-steps.sh $step > /root/rk-$step.log 2>&1; echo \$? > /root/rk-$step.status'"
    deadline=$(($(date +%s) + $(limit "$step")))
    status=hang
    while [ "$(date +%s)" -lt "$deadline" ] && kill -0 "$qemu" 2> /dev/null; do
        sleep 30
        if finished=$(guest "cat /root/rk-$step.status 2> /dev/null") && [ -n "$finished" ]; then
            status=$finished
            break
        fi
    done
    kill -0 "$qemu" 2> /dev/null || status=died
    guest "cat /root/rk-$step.log" > "$out/$step.log" 2> /dev/null
    tail -n 80 "$out/$step.log"
    sed -n 's/^RK-RESULT \([^ ]*\) \(.*\)$/\1\t\2/p' "$out/$step.log" | while IFS=$'\t' read -r k v; do
        record "$step" "$k" "$v"
    done
    if [ "$step" = nginx ] && [ "$status" = 0 ]; then
        want=$(sed -n 's/^RK-RESULT big-sha256 //p' "$out/$step.log")
        got=$(curl -fsS http://127.0.0.1:8080/rk/big | sha256sum | cut -d' ' -f1)
        [ "$got" = "$want" ] || { record nginx failed big-digest; status=1; }
        # QEMU's user network resets a connection now and then under load, and ab gives up on the
        # first reset without the -r, so a round that ended early printed no rate and no failed
        # count and went by as a pass. With -r it carries on and counts the reset. A round with no
        # rate at all is still a failed one, and so is one where more than one request in a hundred
        # failed. The count is recorded either way, so the two kernels can be compared.
        for round in 1 2 3; do
            ab -q -r -n 20000 -c 8 http://127.0.0.1:8080/rk/small.html > "$work/ab" 2>&1
            cat "$work/ab" >> "$out/nginx.log"
            rps=$(awk '/^Requests per second/ { print $4 }' "$work/ab")
            record nginx nginx-virtio-rps "$rps"
            bad=$(awk '/^Failed requests/ { f = $3 } /^Non-2xx responses/ { n = $3 }
                END { print f + n }' "$work/ab")
            record nginx nginx-virtio-failed "$bad"
            if [ -z "$rps" ] || [ "$bad" -gt 200 ]; then
                record nginx failed "ab-round-$round"
                status=1
            fi
            ab -q -n 400 -c 8 http://127.0.0.1:8080/rk/big > "$work/ab" 2>&1
            cat "$work/ab" >> "$out/nginx.log"
            record nginx nginx-virtio-big-rps "$(awk '/^Requests per second/ { print $4 }' "$work/ab")"
        done
    fi
    record "$step" status "$status"
    [ "$status" = 0 ] || failed=1
    echo "::endgroup::"
    echo "$step: $status"
    [ "$status" = died ] && break
done

if kill -0 "$qemu" 2> /dev/null; then
    guest dmesg > "$out/dmesg.txt" 2> /dev/null
    guest systemctl poweroff 2> /dev/null
    for _ in $(seq 60); do
        kill -0 "$qemu" 2> /dev/null || break
        sleep 2
    done
    kill "$qemu" 2> /dev/null
fi
wait "$qemu"
exit $failed
