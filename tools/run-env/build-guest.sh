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
ALPINE="${ALPINE:-3.22}"
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

# The package-signing key's PUBLIC half goes into the image (3e). Created on
# first use; see package-key.sh for where the private half lives and why.
KEYNAME="$("$HERE/package-key.sh")"

podman run --rm --arch 386 \
  -v "${HERE}/fs2json.py:/fs2json.py:ro,z" \
  -v "${HERE}/packs.txt:/packs.txt:ro,z" \
  -v "${HERE}/.keys/${KEYNAME}:/entity-packages.pub:ro,z" \
  -e KEYNAME="$KEYNAME" \
  -v "${OUT}/guest:/guest:z" \
  -e SOURCE_DATE_EPOCH \
  -e PKGS="$PKGS" \
  -e GUEST_INIT="${GUEST_INIT:-entity}" \
  "$IMAGE" sh -euc '
    apk add --no-cache mkinitfs python3 gcc musl-dev linux-headers >/dev/null 2>&1

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
      echo "::respawn:/usr/local/sbin/entity-agent" >> /out/etc/inittab
      echo "ttyS0::respawn:/usr/bin/env HOME=/root /bin/bash -l" >> /out/etc/inittab
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
# The person working files, put on the disk by the page BEFORE the machine
# started (build-guest.sh 3d). Extracted here, before any shell exists, so a
# restored ~/.bash_history is the one bash reads.
if [ -f /var/lib/entity/restore.tar ]; then
  if tar -xpf /var/lib/entity/restore.tar -C /root 2>/run/entity-restore.err; then
    echo ok > /run/entity-restore.status
  else
    echo failed > /run/entity-restore.status
  fi
  rm -f /var/lib/entity/restore.tar
fi
cat /etc/motd 2>/dev/null
SYSINIT
    chmod 0755 /out/sbin/entity-sysinit
    cat > /out/etc/inittab <<"INITTAB"
# Alpine Environment -- no OpenRC. See build-guest.sh 3a.
::sysinit:/sbin/entity-sysinit
::respawn:/usr/local/sbin/entity-agent
ttyS0::respawn:/usr/bin/env HOME=/root /bin/bash -l
::ctrlaltdel:/sbin/reboot
::shutdown:/bin/umount -a -r
INITTAB
    fi

    # HOME=/root in inittab, not only here: busybox init gives every process it
    # starts HOME=/ (login(1) would set it, and there is no getty), and bash
    # decides where its history file lives before it reads any profile. So `~`
    # was `/`, history went to /.bash_history, and none of it was ever saved --
    # found when a `~/file` written in the guest never reached the workspace.
    cat > /out/etc/profile.d/10-entity.sh <<"PROFILE"
# $SHELL is set by login(1), and we deliberately have no getty -- so without
# this it reads /bin/sh and anything that re-execs "$SHELL" drops out of bash.
export SHELL=/bin/bash
# Start in the home directory, which is the directory that is kept. login(1)
# would do this; init starts the shell in /. HOME is set by inittab (3a).
export HOME=/root
[ "$PWD" = / ] && cd "$HOME"
# The console shell (started by init, so its parent is init) says who it is,
# and after a snapshot resume the page presses one key sequence bound to
# __entity_resumed: load the history restored into ~ and reseed $RANDOM, both
# of which this shell took from the moment the snapshot was built
# (build-guest.sh 3d, the `shell` verb). The marker file is how the agent knows
# it ran rather than assuming it did.
if [ "$PPID" = 1 ]; then
  mkdir -p /run/entity && printf %s "$$" > /run/entity/shell.pid
  __entity_resumed() {
    history -r
    # Readline is mid-line while this runs and afterwards writes the empty
    # line it holds over the history entry at the position it started from --
    # which history -r just filled with the first restored line (measured).
    # Adding and deleting an entry moves that position past the end.
    history -s __entity_resumed; history -d -1
    RANDOM=$(( $(od -An -N2 -tu2 /dev/urandom) ))
    : > /run/entity/shell-refreshed
  }
  case $- in *i*) bind -x "\"\e[9999~\": __entity_resumed" ;; esac
fi
export PS1="\[\e[1;36m\]\w\[\e[0m\] \[\e[1;32m\]\$\[\e[0m\] "
export TERM="${TERM:-xterm-256color}"
export PAGER=less LESS=-R
alias ls="ls --color=auto"; alias grep="grep --color=auto"
alias ll="ls -alF"; alias la="ls -A"
# Write history at every prompt, not at exit: this shell never exits, so
# otherwise nothing a person typed would ever reach ~/.bash_history to be kept.
# Then tell the page a command finished (an OSC it swallows, never drawn): it
# saves the home directory a moment later, so closing the window loses seconds
# of work, not the 30 s between autosaves. The Apps window gives an app no
# warning before it closes it, so this is the save that runs before a close.
__entity_prompt() { history -a; printf "\033]5379;prompt\007"; }
export PROMPT_COMMAND=__entity_prompt
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
    mkdir -p /out/usr/local/bin /out/usr/local/sbin /out/usr/local/lib /out/var/lib/entity
    # Shared by send and save: wait for the page to answer on the control
    # line (3d). The answer is a file the agent writes, so the person sees
    # what the HOST did, not a line printed before anyone had done anything.
    cat > /out/usr/local/lib/entity-wait.sh <<"WAIT"
# entity_token -- 8 hex characters naming one request.
entity_token() { od -An -N4 -tx4 /dev/urandom | tr -d " \n"; }
# entity_wait TOKEN SECONDS -- print the page answer; exit status says ok/fail.
entity_wait() {
  f=/run/entity/result.$1; i=0
  while [ "$i" -lt $(( $2 * 10 )) ]; do
    if [ -f "$f" ]; then
      read -r st msg < "$f"; rm -f "$f"; echo "$msg"
      [ "$st" = ok ]; return
    fi
    sleep 0.1; i=$(( i + 1 ))
  done
  return 2
}
WAIT
    cat > /out/usr/local/bin/send <<"SEND"
#!/bin/sh
# send FILE...  -- hand files to the page hosting this machine. In the entity
# browser they are kept in your files (File Transfer); opened on its own, the
# page downloads them instead. See build-guest.sh 3c.
. /usr/local/lib/entity-wait.sh
[ $# -gt 0 ] || { echo "usage: send FILE..." >&2; exit 2; }
sync
rc=0
for f in "$@"; do
  if [ ! -f "$f" ]; then echo "send: $f: not a regular file" >&2; rc=1; continue; fi
  p=$(realpath "$f")
  case "$p" in *[[:cntrl:]]*) echo "send: $f: name holds a control character" >&2; rc=1; continue;; esac
  tok=$(entity_token)
  printf "\033]5379;send;%s;%s\007" "$tok" "$p"
  entity_wait "$tok" 60
  case $? in
    0) ;;
    1) rc=1 ;;
    *) echo "send: $p: no answer from the page -- it may not have kept the file" >&2; rc=1 ;;
  esac
done
exit $rc
SEND
    cat > /out/usr/local/bin/receive <<"RECEIVE"
#!/bin/sh
# receive  -- ask the page for a file. It lands in your home directory under
# its own name (never over a file already there) once you pick one with the
# host "Send a file" button, and is kept with the rest of the home directory.
# See build-guest.sh 3c.
printf "\033]5379;receive\007"
echo "receive: asked the page for a file; it will appear in your home directory (~)"
RECEIVE
    cat > /out/usr/local/bin/save <<"SAVE"
#!/bin/sh
# save  -- keep the files in /root now. The page also does this on its own
# after each command, every 30 seconds, and when you leave. See build-guest.sh 3d.
. /usr/local/lib/entity-wait.sh
tok=$(entity_token)
printf "\033]5379;save;%s\007" "$tok"
entity_wait "$tok" 300
case $? in
  0) exit 0 ;;
  1) exit 1 ;;
  *) echo "save: no answer from the page" >&2; exit 1 ;;
esac
SAVE
    chmod 0755 /out/usr/local/bin/send /out/usr/local/bin/receive /out/usr/local/bin/save

    # ---- 3c3. packs: ready-made tool sets, one command each ----------------
    # The package set is over a hundred names; a person who wants "C" should not
    # have to know it is tcc + tcc-libs-static + musl-dev + make (and that tcc cannot
    # link without tcc-libs-static). packs.txt is the list; build-about.py fails the build if a pack
    # names anything that cannot be installed offline.
    mkdir -p /out/usr/local/share/entity
    grep -v "^#" /packs.txt | grep -v "^[[:space:]]*$" > /out/usr/local/share/entity/packs.txt
    cat > /out/usr/local/bin/packs <<"PACKS"
#!/bin/sh
# packs -- ready-made tool sets. Everything installs from this site, no network.
F=/usr/local/share/entity/packs.txt
field() { printf %s "$1" | cut -d"|" -f"$2" | sed "s/^ *//; s/ *$//"; }
names_of() { while IFS= read -r l; do [ "$(field "$l" 1)" = "$1" ] && field "$l" 3; done < "$F"; }
case "${1:-}" in
  ""|list|-h|--help)
    echo "Ready-made tool sets. Install one with:  packs add NAME   (several: packs add c data)"
    echo
    while IFS= read -r l; do printf "  %-10s %s\n" "$(field "$l" 1)" "$(field "$l" 2)"; done < "$F"
    echo
    echo "  packs show NAME   prints the apk add line, to install part of a set."
    echo "  Installed tools last until the machine restarts; your home directory is kept."
    ;;
  show)
    shift; [ $# -gt 0 ] || { echo "packs show NAME" >&2; exit 2; }
    for p in "$@"; do n=$(names_of "$p"); [ -n "$n" ] || { echo "packs: no pack called $p (packs lists them)" >&2; exit 1; }; echo "apk add $n"; done ;;
  add|install)
    shift; [ $# -gt 0 ] || { echo "packs add NAME..." >&2; exit 2; }
    all=""
    for p in "$@"; do n=$(names_of "$p"); [ -n "$n" ] || { echo "packs: no pack called $p (packs lists them)" >&2; exit 1; }; all="$all $n"; done
    echo "apk add$all"
    exec apk add $all ;;
  *) echo "packs: unknown command $1 -- try: packs" >&2; exit 2 ;;
esac
PACKS
    chmod 0755 /out/usr/local/bin/packs

    # ---- 3c4. tcc links: the crt files where the x86 package looks ---------
    # The Alpine 3.22 x86 tcc searches /usr/lib/i386-linux-gnu for crt1.o, crti.o and
    # crtn.o (a Debian multiarch path), so `tcc -run` works and `tcc -o` fails with
    # "library crt1.o not found". Verified on 3.22.5 x86: these three links make it
    # link and run. They dangle until musl-dev is installed, which is what `packs
    # add c` does; a dangling link costs nothing.
    mkdir -p /out/usr/lib/i386-linux-gnu
    for f in crt1.o crti.o crtn.o; do ln -sf ../$f /out/usr/lib/i386-linux-gnu/$f; done

    # ---- 3c2. reboot / poweroff / halt / shutdown: the PAGE does them ------
    # This machine lives in a page. A reboot inside the guest cannot work after
    # a snapshot resume (the kernel was never fetched, so there is nothing to
    # boot), and a poweroff inside it leaves a dead terminal and an unsaved home
    # directory. So these names, first on PATH, ask the page instead: it saves
    # the home directory, then restarts the machine or turns it off.
    # No page answers within 5 s (the headless boot gate, an older page): the
    # real busybox command runs. Busybox has no `shutdown`; this one reads -r.
    cat > /out/usr/local/sbin/reboot <<"POWER"
#!/bin/sh
# reboot / poweroff / halt / shutdown [-r|-h]  -- ask the page hosting this
# machine to restart or stop it, after saving the home directory.
. /usr/local/lib/entity-wait.sh
me=$(basename "$0")
verb=off
case "$me" in reboot) verb=restart ;; esac
if [ "$me" = shutdown ]; then
  for a in "$@"; do case "$a" in -r*) verb=restart ;; esac; done
fi
tok=$(entity_token)
printf "\033]5379;power;%s;%s\007" "$tok" "$verb"
if entity_wait "$tok" 5; then exit 0; fi
if [ "$me" = shutdown ]; then
  [ "$verb" = restart ] && exec /sbin/reboot
  exec /sbin/poweroff
fi
exec "/sbin/$me" "$@"
POWER
    chmod 0755 /out/usr/local/sbin/reboot
    for n in poweroff halt shutdown; do ln -sf reboot "/out/usr/local/sbin/$n"; done

    # ---- 3d. the control line: ttyS1, which no person sees -----------------
    # The page used to tell the shell its window size by TYPING "stty rows R
    # cols C" into it. That echoed on every resize, and inside vi or nano it
    # typed those words into the file. So the page and the guest talk on a
    # second serial port instead, and this agent does what the page asks:
    #   size R C        stty the CONSOLE (a SIGWINCH reaches whatever is open)
    #   result TOK ...  the answer to a send or save, for the waiting script
    #   manifest TOK    list /root (mode mtime size path) for the page to diff
    #   zerofill TOK    BUILD TIME ONLY: zero free memory so a snapshot compresses
    #   resume TOK EPOCH SEED ARCHIVE   after a snapshot restore: set the clock,
    #                   reseed the kernel RNG, extract the saved /root archive,
    #                   (then the page asks `shell`)
    #   shell TOK       after a resume: wait for the console shell to confirm
    #                   it refreshed itself, and replace it if it does not
    #   receive TOK STAGED NAME64   move a file the page put on the disk into
    #                   /root under its own name, never over an existing file
    # It answers "hello" when it starts, "manifest TOK N", "zeroed TOK",
    # "resumed TOK FILES RNG", "shell TOK STATUS" and "received TOK STATUS
    # [NAME64]". It never runs
    # anything the page names: the verbs and
    # every path they touch are fixed here; arguments are checked character by
    # character before use.
    # entity-reseed: stdin bytes -> RNDADDENTROPY (credited) -> RNDRESEEDCRNG.
    # Built here because no shell tool can make either ioctl. Static, so it
    # needs nothing from the guest. Used by the agent resume verb.
    cat > /tmp/entity-reseed.c <<"RESEED"
#include <fcntl.h>
#include <linux/random.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>
int main(void) {
    struct { int entropy_count; int buf_size; unsigned char buf[512]; } req;
    ssize_t n = read(0, req.buf, sizeof req.buf);
    if (n < 16) return 2;
    req.entropy_count = (int)n * 8;
    req.buf_size = (int)n;
    int fd = open("/dev/urandom", O_WRONLY);
    if (fd < 0) return 3;
    if (ioctl(fd, RNDADDENTROPY, &req) < 0) return 4;
    if (ioctl(fd, RNDRESEEDCRNG) < 0) return 5;
    return 0;
}
RESEED
    gcc -O2 -static -s -o /out/usr/local/sbin/entity-reseed /tmp/entity-reseed.c
    chmod 0755 /out/usr/local/sbin/entity-reseed

    cat > /out/usr/local/sbin/entity-agent <<"AGENT"
#!/bin/sh
# entity-agent -- the page side of the machine, on ttyS1. See build-guest.sh 3d.
set -f
TTY=/dev/ttyS1
[ -c "$TTY" ] || { sleep 60; exit 1; }
# clocal FIRST: a serial port without it blocks open() waiting for a carrier.
stty -F "$TTY" clocal -echo -onlcr 2>/dev/null || { sleep 60; exit 1; }
mkdir -p /run/entity /var/lib/entity
exec 3<>"$TTY"
printf "hello\n" >&3
while IFS= read -r line <&3; do
  set -- $line
  case "$1" in
    size)
      case "$2$3" in ""|*[!0-9]*) continue ;; esac
      stty -F /dev/ttyS0 rows "$2" cols "$3" 2>/dev/null ;;
    result)
      case "$2" in ""|*[!0-9a-f]*) continue ;; esac
      tok=$2; shift 2
      printf "%s\n" "$*" > "/run/entity/result.$tok.tmp" && mv "/run/entity/result.$tok.tmp" "/run/entity/result.$tok" ;;
    manifest)
      case "$2" in ""|*[!0-9a-f]*) continue ;; esac
      sync
      ( cd /root && find . -type f ! -path "./.cache/*" -exec stat -c "%a %Y %s %n" {} + ) \
        > /var/lib/entity/manifest.tmp 2>/dev/null
      mv /var/lib/entity/manifest.tmp /var/lib/entity/manifest
      sync
      printf "manifest %s %s\n" "$2" "$(wc -l < /var/lib/entity/manifest)" >&3 ;;
    zerofill)
      # Freed pages keep their old contents, so a snapshot of a booted machine
      # stores megabytes of garbage. Fill free memory with zeros once, then free
      # it: v86 omits all-zero memory from a state. 3 MiB of headroom, measured
      # (PLAN-2026-09-13 §6): 12 MiB left 13.1 MiB compressed, 3 MiB 10.5.
      case "$2" in ""|*[!0-9a-f]*) continue ;; esac
      sync; echo 3 > /proc/sys/vm/drop_caches
      mkdir -p /run/zf && mount -t tmpfs -o size=250m tmpfs /run/zf 2>/dev/null
      n=$(( $(awk "/MemFree/ {print \$2}" /proc/meminfo) / 1024 - 3 ))
      dd if=/dev/zero of=/run/zf/z bs=1M count=$n 2>/dev/null
      rm -f /run/zf/z; umount /run/zf 2>/dev/null
      printf "zeroed %s\n" "$2" >&3 ;;
    resume)
      # A restored snapshot wakes with the clock, the RNG state and the files of
      # the moment it was taken, for EVERY visitor. Each is put right here,
      # before the page hands the terminal over.
      case "$2" in ""|*[!0-9a-f]*) continue ;; esac
      case "$3" in ""|*[!0-9]*) continue ;; esac
      case "$4" in ""|*[!A-Za-z0-9+/=]*) continue ;; esac
      case "$5" in -|restore-[0-9a-f]*.tar) ;; *) continue ;; esac
      date -u -s "@$3" >/dev/null 2>&1
      # Credited entropy AND a forced reseed (entity-reseed): writing to
      # /dev/urandom alone only mixes, and the kernel would keep handing out
      # the snapshot`s stream until its next scheduled reseed (up to 60 s).
      rng=reseeded
      printf %s "$4" | base64 -d 2>/dev/null | /usr/local/sbin/entity-reseed || rng=reseed-failed
      st=none
      if [ "$5" != - ] && [ -f "/var/lib/entity/$5" ]; then
        if tar -xpf "/var/lib/entity/$5" -C /root 2>/run/entity-restore.err; then st=ok; else st=failed; fi
        echo "$st" > /run/entity-restore.status
        rm -f "/var/lib/entity/$5"
      elif [ "$5" != - ]; then
        st=missing
      fi
      cat /etc/motd > /dev/ttyS0 2>/dev/null
      # The page now brings the console shell up to date (the `shell` verb).
      rm -f /run/entity/shell-refreshed
      printf "resumed %s %s %s\n" "$2" "$st" "$rng" >&3 ;;
    shell)
      # After a resume: did the console shell refresh itself? The shell was
      # started at BUILD time, so it holds none of the restored ~/.bash_history
      # and the $RANDOM sequence every visitor shares. The page sends it one
      # key sequence bound to a function that fixes both (profile, 3) -- a
      # binding, not a command: nothing reaches the command line. A signal
      # cannot do it (bash does not run a trap while it waits at the prompt,
      # measured). If the shell has not confirmed within 1 s, replace it: init
      # respawns a fresh one, which costs its 1.2 s startup on the emulated CPU.
      # KILL, not HUP: a hung-up bash writes its history over the restored file.
      case "$2" in ""|*[!0-9a-f]*) continue ;; esac
      i=0
      while [ "$i" -lt 20 ] && [ ! -e /run/entity/shell-refreshed ]; do sleep 0.05; i=$(( i + 1 )); done
      if [ -e /run/entity/shell-refreshed ]; then sh_st=refreshed
      else
        pid=$(cat /run/entity/shell.pid 2>/dev/null)
        case "$pid" in
          ""|*[!0-9]*) sh_st=unknown ;;
          *) if kill -KILL "$pid" 2>/dev/null; then sh_st=replaced; else sh_st=unknown; fi ;;
        esac
      fi
      printf "shell %s %s\n" "$2" "$sh_st" >&3 ;;
    receive)
      # The page wrote the file under a fresh staging name (a name the guest
      # never looked up, since the root is cache=loose) and names it here in
      # base64, so a space or a non-ASCII name crosses this line intact.
      case "$2" in ""|*[!0-9a-f]*) continue ;; esac
      case "$3" in incoming-[0-9a-f]*) ;; *) continue ;; esac
      case "$3" in *[!0-9a-z-]*) continue ;; esac
      case "$4" in ""|*[!A-Za-z0-9+/=]*) continue ;; esac
      src="/var/lib/entity/$3"
      name=$(printf %s "$4" | base64 -d 2>/dev/null)
      case "$name" in ""|.|..|*/*|*[[:cntrl:]]*)
        rm -f "$src"; printf "received %s bad-name\n" "$2" >&3; continue ;;
      esac
      [ -f "$src" ] || { printf "received %s missing\n" "$2" >&3; continue; }
      # notes.txt, then notes-1.txt, notes-2.txt: never over a file already
      # there, because the home directory is kept and an overwrite is a loss.
      stem=${name%.*}; ext=${name#"$stem"}
      [ -z "$stem" ] && { stem=$name; ext=; }
      dest=$name; i=1
      while [ -e "/root/$dest" ] || [ -L "/root/$dest" ]; do
        dest="$stem-$i$ext"; i=$(( i + 1 ))
      done
      if mv "$src" "/root/$dest" && chmod 0644 "/root/$dest"; then
        printf "\r\nreceive: %s is in your home directory as ~/%s\r\n" "$name" "$dest" > /dev/ttyS0 2>/dev/null
        printf "received %s ok %s\n" "$2" "$(printf %s "$dest" | base64 | tr -d "\n")" >&3
      else
        rm -f "$src"; printf "received %s failed\n" "$2" >&3
      fi ;;
  esac
done
sleep 1
AGENT
    chmod 0755 /out/usr/local/sbin/entity-agent

    cat > /out/etc/motd <<"MOTD"

  The Alpine Environment -- a real Linux machine, in a browser tab.
  Your home directory (~) is kept in this browser. Everything else starts fresh on restart.
  send FILE   hands a file out to the page    receive   asks the page for one (it lands in ~)
  save        saves ~ now (it also saves after each command and every 30s)
  apk add NAME   installs from the package set this site hosts (apk search lists it), until restart.
  packs          ready-made tool sets: C, scripting, data, media, documents, writing, games...
  reboot / poweroff   restart or turn off the machine -- or use the power button, top right.

  DEMO POSTURE: root, no password, no getty. Not a multi-user box.

MOTD

    # ---- 3e. the package set: a local repository the page mounts ----------
    # build-packages.sh builds a signed apk repository that the page merges
    # into this root at /var/cache/entity-packages when the host offers the
    # `packages` bundle. apk reads it like any repository and the 9p fault
    # fetches only the files an install touches. Trust exactly our key for it,
    # beside the Alpine keys the image already carries (the .apk files inside
    # keep their Alpine signatures). With no bundle the directory is absent and
    # apk says the repository is missing -- a stated outcome, not a hang.
    cp /entity-packages.pub "/out/etc/apk/keys/$KEYNAME"
    chmod 0644 "/out/etc/apk/keys/$KEYNAME"
    echo "/var/cache/entity-packages" > /out/etc/apk/repositories
    echo "    apk: trusts $KEYNAME for /var/cache/entity-packages"

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
