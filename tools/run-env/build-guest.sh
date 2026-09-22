#!/usr/bin/env bash
# build-guest.sh -- build the Alpine Environment guest for v86, end to end.
#
#   ./build-guest.sh [outdir]        default: ./alpine-guest
#
# Produces  <out>/guest/{vmlinuz,initramfs,fs.json,blobs/}  and symlinks the v86
# engine + BIOS beside them, so `node <out>/boot.js` boots it headlessly.
#
# WHY EACH STEP IS HERE (every one of these cost a measurement to learn):
#
#  * --arch 386 i386/alpine.  A 32-bit Alpine container runs NATIVELY on an
#    x86_64 host, so mkinitfs, apk and depmod all run on their own architecture.
#    Cross-building the initramfs from an x86_64 container is where this gets
#    fiddly, and it is entirely avoidable.
#
#  * mkinitfs -F "base 9p virtio".  Alpine ships a `9p` mkinitfs feature out of
#    the box (/etc/mkinitfs/features.d/9p.modules), so the initramfs is a
#    SUPPORTED path, not a hand-rolled cpio. This matters because CONFIG_9P_FS=m
#    in every stock Alpine kernel -- there is no kernel you can boot with
#    root=9p and be done, and that is the step this whole script exists to take.
#
#  * root=host9p.  v86's 9p mount tag, read out of the engine
#    (kd()'s configspace_tagname = [104,111,115,116,57,112]), not guessed.
#
#  * rm -rf /dev /proc /sys.  A 9p root gets these from the guest kernel. Shipping
#    them means device nodes in the index, which fs2json cannot represent and the
#    engine asserts on.
#
#  * ttyS0 gets a LOGIN-FREE bash.  v86 gives us a serial port and no password
#    database worth having; a getty prompt is a wall between a visitor and the
#    thing we are demonstrating. This is a DEMO posture and is called out in the
#    motd -- do not carry it into anything multi-user.
set -euo pipefail

OUT="$(cd "$(dirname "$0")" && pwd)/${1:-alpine-guest}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ALPINE="${ALPINE:-3.21}"
IMAGE="docker.io/i386/alpine:${ALPINE}"
# Reproducible builds: a fixed mtime keeps fs.json byte-stable across runs, so a
# rebuilt guest that changed nothing has the same index hash and every consumer
# holding it re-fetches nothing.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1757600000}"

# The comfortable profile (DESIGN-2026-09-11-h §2): bash, real coreutils and a
# terminfo that makes colour work. 6 MiB over minimal, and the whole reason the
# stock busybox shell "kind of sucks".
PKGS="${PKGS:-alpine-base bash bash-completion ncurses ncurses-terminfo coreutils findutils grep sed less nano}"

echo "==> building Alpine ${ALPINE} guest (x86) into ${OUT}"
# CLEAN ONLY WHAT THIS SCRIPT WRITES. `rm -rf "$OUT"` was the original and the
# default OUT is ./alpine-guest -- which holds index.html (THE page, 29 KB of
# hand-won terminal/keyboard/contract work), host.html and boot.js, none of
# which this script can regenerate. So the documented first command in the
# handoff, `./build-guest.sh` with no argument, destroyed the most valuable
# artifact in the rig. Never fired: nobody had run it as documented since the
# page was written. The guest/ subtree is the only thing that is ours to drop.
rm -rf "$OUT/guest"; mkdir -p "$OUT/guest"

podman run --rm --arch 386 \
  -v "${HERE}/fs2json.py:/fs2json.py:ro,z" \
  -v "${OUT}/guest:/guest:z" \
  -e SOURCE_DATE_EPOCH \
  -e PKGS="$PKGS" \
  -e GUEST_INIT="${GUEST_INIT:-entity}" \
  "$IMAGE" sh -euc '
    apk add --no-cache mkinitfs python3 >/dev/null 2>&1

    # ---- 1. the root filesystem + the kernel --------------------------------
    apk add --root /out --initdb --no-cache --keys-dir /etc/apk/keys \
        --repositories-file /etc/apk/repositories $PKGS linux-virt >/dev/null 2>&1
    KV=$(ls /out/lib/modules | head -1)
    echo "    kernel  $KV"

    # ---- 2. the initramfs that can mount a 9p root -------------------------
    mkinitfs -b /out -F "base 9p virtio" -k "$KV" >/dev/null 2>&1
    cp /out/boot/vmlinuz-virt  /guest/vmlinuz

    # ---- 2b. HALF THE BOOT TIME: do not hunt for a device that cannot exist -
    # Alpine`s init runs `nlplug-findfs $KOPT_root` before mounting, to plug and
    # settle the block device the root lives on. `host9p` is a 9p MOUNT TAG, not
    # a device, so the hunt can only ever time out -- MEASURED at 7.5s of a 15.5s
    # boot, i.e. half of it, spent looking for something that does not exist.
    # A/B, same image, only this patch: 15.5s -> 8.0s to a bash prompt.
    mkdir -p /tmp/ir && cd /tmp/ir
    gzip -dc /out/boot/initramfs-virt | cpio -idm 2>/dev/null
    # Anchored on ONE line and guarding with `||`, deliberately: the command is
    # three lines with backslash continuations, and a multi-line anchor has to be
    # escaped through a shell heredoc AND a python string -- which is how the
    # first cut of this patch silently matched nothing.
    python3 - <<"PATCH"
anchor = "$MOCK nlplug-findfs $cryptopts"
s = open("init").read()
assert anchor in s, "init anchor moved -- Alpine changed its init, go and re-read it"
open("init", "w").write(s.replace(anchor, \
    "[ \"$rootfstype\" = 9p ] && ebegin \"9p root is a tag, not a device -- skipping the hunt\" && eend 0 || " \
    + anchor, 1))
print("    patched init: nlplug-findfs skipped for a 9p root")
PATCH
    # ---- 2c. REPRODUCIBILITY: the bodies were never the problem -------------
    # A/B of two builds, same inputs: ZERO of 147 file BODIES differ and the
    # archive hash moves anyway. Measured, per cpio newc header field:
    #   ino     147/147 -- host inode numbers, fresh every container run
    #   devmin  147/147 -- minor number of the backing filesystem
    #   mtime    54/147 -- the files THIS build creates (extract, then patch)
    # plus gzip, which stamps its own mtime into the header when fed a pipe.
    # SOURCE_DATE_EPOCH is exported for fs2json but nothing here consulted it.
    # Unfixed this silently defeats the thing the epoch exists for: the
    # initramfs is 5.1 MB of a 21.4 MiB first visit, so an unchanged rebuild
    # re-addresses it and every consumer holding a copy fetches it again.
    # `sort` is belt-and-braces -- the find order matched across both builds,
    # but that is the filesystem agreeing with itself, not a guarantee.
    # (NB: no apostrophes in this block. The whole body is a single-quoted
    # argument to `sh -euc`, so one contraction ends the string 60 lines early
    # and the failure surfaces as a redirect into a directory that "does not
    # exist" -- which is how the first cut of this comment broke the build.)
    find . -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
    find . | sort | cpio -o -H newc --renumber-inodes --ignore-devno 2>/dev/null \
      | gzip -9n > /guest/initramfs
    cd /
    chmod 0644 /guest/vmlinuz /guest/initramfs
    echo "    vmlinuz $(stat -c %s /guest/vmlinuz) bytes / initramfs $(stat -c %s /guest/initramfs) bytes"

    # ---- 3. make it a usable environment rather than a login wall ----------
    # bash for root, straight onto the serial console, no getty.
    sed -i "s|^root:x:0:0:root:/root:.*|root:x:0:0:root:/root:/bin/bash|" /out/etc/passwd
    if [ "$GUEST_INIT" = openrc ]; then
      sed -i "/ttyS0/d" /out/etc/inittab
      echo "ttyS0::respawn:/bin/bash -l" >> /out/etc/inittab
    else
    # ---- 3a. NO OPENRC: busybox init, one sysinit script, one shell --------
    # MEASURED (serial as the console, timestamped): of ~9.7s between mounting
    # the root and a prompt, OpenRC is at least 6 -- 4.0s of it on "Caching
    # service dependencies" alone, i.e. sourcing every init script over 9p on an
    # emulated CPU. On an Android phone that stretch was ~45s with nothing on
    # screen, because OpenRC prints to tty0 (the hidden VGA console) and the
    # serial terminal a person is watching goes silent: it read as a freeze.
    # A single-user demo guest wants none of the services: no network, no
    # syslog, no hwclock, no modules (there are none, see 4b), and ttyS0 gets
    # its shell from busybox init directly. So keep busybox init -- it opens
    # the tty, sets it as the controlling terminal, respawns, and reaps -- and
    # replace only the OpenRC lines with the mounts a shell actually needs.
    # The tty1..6 gettys go too: they run on the VGA console nobody can see.
    # GUEST_INIT=openrc rebuilds the previous guest for an A/B.
    cat > /out/sbin/entity-sysinit <<"SYSINIT"
#!/bin/sh
# The whole of userland startup for the Alpine Environment. See build-guest.sh 3a.
mountpoint -q /proc    || mount -t proc     proc     /proc
mountpoint -q /sys     || mount -t sysfs    sysfs    /sys
mountpoint -q /dev     || mount -t devtmpfs devtmpfs /dev
mkdir -p /dev/pts /dev/shm /run /tmp
mountpoint -q /dev/pts || mount -t devpts   devpts   /dev/pts -o gid=5,mode=620
mountpoint -q /dev/shm || mount -t tmpfs    shm      /dev/shm -o mode=1777,nosuid,nodev
mountpoint -q /run     || mount -t tmpfs    run      /run     -o mode=0755,nosuid,nodev
[ -r /etc/hostname ] && hostname -F /etc/hostname
cat /etc/motd 2>/dev/null
SYSINIT
    chmod 0755 /out/sbin/entity-sysinit
    cat > /out/etc/inittab <<"INITTAB"
# Alpine Environment -- no OpenRC. See build-guest.sh 3a.
::sysinit:/sbin/entity-sysinit
ttyS0::respawn:/bin/bash -l
::ctrlaltdel:/sbin/reboot
::shutdown:/bin/umount -a -r
INITTAB
    fi

    cat > /out/etc/profile.d/10-entity.sh <<"PROFILE"
# $SHELL is set by login(1), and we deliberately have no getty -- so without
# this it reads /bin/sh and anything that re-execs "$SHELL" drops out of bash.
export SHELL=/bin/bash
export PS1="\[\e[1;36m\]\w\[\e[0m\] \[\e[1;32m\]\$\[\e[0m\] "
export TERM="${TERM:-xterm-256color}"
export PAGER=less LESS=-R
alias ls="ls --color=auto"; alias grep="grep --color=auto"
alias ll="ls -alF"; alias la="ls -A"
# The motd, once per boot, on the terminal a person is actually looking at: with
# no getty and no OpenRC nothing else prints it, and it is the only place the
# send/receive verbs are discoverable.
if [ -t 0 ] && [ ! -e /run/motd-shown ]; then cat /etc/motd; : > /run/motd-shown; fi
PROFILE
    chmod 0644 /out/etc/profile.d/10-entity.sh

    # ---- 3b. THE CLOCK APK LEAVES IN /etc/shadow ---------------------------
    # Field 3 of a shadow entry is "days since epoch when the password last
    # changed", and apk writes it from TODAY when a package creates a user. So
    # the root filesystem carries the build DATE: klogd read 20707 on
    # 2026-09-11 and 20708 on 2026-09-12, which moved /etc/shadow, which moved
    # its content hash, which moved fs.json -- an unchanged guest that
    # re-addresses itself at midnight UTC and makes every consumer refetch.
    # SOURCE_DATE_EPOCH cannot reach it; nothing consults the variable here.
    # FOUND LATE, AND BY ACCIDENT: the reproducibility check in
    # MEASUREMENT-...-r ran three builds ON ONE DAY, so the one variable this
    # depends on was held still by the rig -- exactly the shape that entry
    # warns about for page stamps. Pin it to the same epoch everything else
    # uses, computed from it rather than typed, so the two cannot drift.
    SDE_DAYS=$(( SOURCE_DATE_EPOCH / 86400 ))
    sed -i "s/^\([^:]*\):\([^:]*\):[0-9][0-9]*:/\1:\2:${SDE_DAYS}:/" /out/etc/shadow
    echo "    pinned /etc/shadow change-dates to ${SDE_DAYS} (SOURCE_DATE_EPOCH)"

    # ---- 3c. send / receive: moving a file across the page boundary ---------
    # The guest cannot postMessage, so it PRINTS: a private OSC sequence,
    # ESC ] 5379 ; verb ; arg BEL, which the page registers with xterm and acts
    # on (index.html, "the guest own file verbs"). Nothing is typed at the
    # shell on the guest behalf, so this is safe with nano open. A path holding
    # a control character is refused: it would end the sequence early.
    mkdir -p /out/usr/local/bin
    cat > /out/usr/local/bin/send <<"SEND"
#!/bin/sh
# send FILE...  -- hand files to the page hosting this machine. In the entity
# browser they are kept as offers under your files; opened on its own, the
# page downloads them instead. See build-guest.sh 3c.
[ $# -gt 0 ] || { echo "usage: send FILE..." >&2; exit 2; }
sync
rc=0
for f in "$@"; do
  if [ ! -f "$f" ]; then echo "send: $f: not a regular file" >&2; rc=1; continue; fi
  p=$(realpath "$f")
  case "$p" in *[[:cntrl:]]*) echo "send: $f: name holds a control character" >&2; rc=1; continue;; esac
  printf "\033]5379;send;%s\007" "$p"
  echo "send: $p ($(wc -c < "$p") bytes) handed to the page"
done
exit $rc
SEND
    cat > /out/usr/local/bin/receive <<"RECEIVE"
#!/bin/sh
# receive  -- ask the page for a file. It lands in /mnt under its own name
# once you pick one with the host "Send a file" button. See build-guest.sh 3c.
printf "\033]5379;receive\007"
echo "receive: asked the page for a file; it will appear in /mnt"
RECEIVE
    chmod 0755 /out/usr/local/bin/send /out/usr/local/bin/receive

    cat > /out/etc/motd <<"MOTD"

  The Alpine Environment -- a real Linux machine, in a browser tab.
  send FILE   hands a file out to the page    receive   asks the page for one (it lands in /mnt)

  DEMO POSTURE: root, no password, no getty. Not a multi-user box.

MOTD

    # ---- 4. a 9p root gets these from the kernel ---------------------------
    rm -rf /out/dev/* /out/proc/* /out/sys/*
    mkdir -p /out/mnt /out/root

    # ---- 4b. DROP THE KERNEL FROM THE SHIPPED ROOT -------------------------
    # linux-virt is installed above ONLY so mkinitfs has modules to pick from.
    # Shipping /lib/modules and /boot in the 9p root costs 37 MiB -- it took the
    # image from 13 to 51 MiB on the first build of this script -- to carry
    # modules the guest never loads, because everything it needs to reach its
    # root is already inside the initramfs.
    # THE TRADE, STATED: with these gone the guest cannot modprobe anything at
    # runtime (loop, fuse, nbd...). If a use for that appears, ship the modules
    # and take the megabytes -- but they fault lazily, so the cost is mostly in
    # the index rather than the transfer.
    rm -rf /out/lib/modules /out/boot
    mkdir -p /out/boot

    # ---- 5. index it: v86 addresses every file body by SHA-256 -------------
    python3 /fs2json.py /out /guest
  '

# RELATIVE links. Absolute ones (the original) name this machine's clone path,
# so the rig broke the moment the tree was seen from anywhere else: a container
# mounting it at another path got `Cannot find module .../build/libv86.js`, and
# so would a moved or re-cloned checkout. Found on the first run from a fresh
# clone, where node is not on the host and the gate runs in a container.
ENGINE_REL="$(realpath --relative-to="${OUT}" "${HERE}/v86-m1")"
ln -sfn "${ENGINE_REL}/build"  "${OUT}/build"
ln -sfn "${ENGINE_REL}/bios"   "${OUT}/bios"
# vendor/ too: index.html loads vendor/xterm.{js,css} and vendor/addon-fit.js,
# and a build into a fresh outdir used to leave those three 404ing.
ln -sfn "${ENGINE_REL}/vendor" "${OUT}/vendor"
[ -f "${HERE}/alpine-guest/boot.js" ] && [ "${OUT}" != "${HERE}/alpine-guest" ] \
  && cp "${HERE}/alpine-guest/boot.js" "${OUT}/boot.js" || true

echo "==> ${OUT}"
echo "    boot it headlessly:  node ${OUT}/boot.js"
