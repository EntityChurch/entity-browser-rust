#!/usr/bin/env bash
# build-kernel.sh -- a kernel for v86 with 9p BUILT IN, so there is no initramfs.
#
#   ./build-kernel.sh [outdir]      default: ./kernel-min
#
# WHY THIS EXISTS
#
# Alpine ships CONFIG_9P_FS=m / CONFIG_NET_9P=m, so a 9p root cannot be mounted
# until something has already loaded those modules -- which is the entire job of
# the 5.15 MB initramfs build-guest.sh constructs. build-guest.sh says so itself:
# "there is no kernel you can boot with root=9p and be done, and that is the
# step this whole script exists to take."
#
# Build them in and the step is not taken, it is DELETED. Measured, against the
# stock Alpine pair:
#
#   vmlinuz-virt      7,680,512      (generic VM kernel, 908 modules configured)
#   initramfs-virt    5,151,033      (exists only to insmod 9p + virtio)
#   ------------------------------
#   fixed cost       12,831,545 B    = 12.2 MiB of a 21.4 MiB first visit
#
# A config carrying x86-32, virtio, 9p and a serial console and close to nothing
# else should land far under that, with no second artifact at all.
set -euo pipefail

OUT="$(cd "$(dirname "$0")" && pwd)/${1:-kernel-min}"
# 6.12.109 deliberately: it is EXACTLY the version Alpine ships as linux-virt,
# so the size comparison is one config against another and not two kernels.
KVER="${KVER:-6.12.109}"
MAJOR="${KVER%%.*}"
# Pinned by hash, because this is layer 3 of the provenance table and the whole
# point of building it ourselves is that we can say which bytes we compiled.
SRC="https://cdn.kernel.org/pub/linux/kernel/v${MAJOR}.x/linux-${KVER}.tar.xz"

mkdir -p "$OUT"
echo "==> kernel ${KVER} for v86 (i686, 9p builtin, no initramfs) -> ${OUT}"

podman run --rm -v "${OUT}:/out:z" -e KVER="$KVER" -e SRC="$SRC" \
  docker.io/library/alpine:3.21 sh -euc '
    apk add --no-cache bash gcc make musl-dev bc flex bison perl linux-headers \
        elfutils-dev openssl-dev openssl-libs-static xz curl diffutils \
        >/dev/null 2>&1

    cd /tmp
    echo "    fetching $SRC"
    curl -sSL -o linux.tar.xz "$SRC"
    echo "    sha256 $(sha256sum linux.tar.xz | cut -d" " -f1)"
    tar xf linux.tar.xz && cd "linux-${KVER}"

    export ARCH=x86
    # tinyconfig is the honest floor: it turns EVERYTHING off, so every option
    # below is one we deliberately asked for and can be asked to justify.
    make tinyconfig >/dev/null 2>&1

    on()  { ./scripts/config --enable  "$1"; }
    off() { ./scripts/config --disable "$1"; }

    # -- 32-bit x86, because the working v86 chain is i686 --------------------
    off 64BIT
    on  X86_32
    on  M686

    # -- a console on the serial port: this IS the terminal -------------------
    on TTY
    on SERIAL_8250
    on SERIAL_8250_CONSOLE
    on SERIAL_8250_PCI
    on PRINTK
    on BINFMT_ELF
    # RX, not TX. Without a working IRQ4 the 8250 still PRINTS -- it just never
    # receives, which presents as a shell that draws its prompt and ignores you.
    # Measured: tinyconfig has no APIC/ACPI/PNP at all, Alpine has all three.
    # X86_LOCAL_APIC/X86_IO_APIC are SELECTED, not set directly. Alpine gets
    # them via SMP; v86 is single-CPU, so the uniprocessor pair is the smaller
    # route to the same interrupt controller.
    on X86_UP_APIC
    on X86_UP_IOAPIC
    on X86_LOCAL_APIC
    on X86_IO_APIC
    on ACPI
    on PNP
    on SERIAL_8250_PNP
    on HIGH_RES_TIMERS
    # The 8250 extras Alpine carries and tinyconfig does not. SHARE_IRQ is the
    # one with a mechanism behind the guess: if the UART IRQ is not accepted as
    # shared the driver runs TX-only, which is exactly the symptom -- a prompt
    # that draws and a shell that never sees a keystroke.
    on SERIAL_8250_EXTENDED
    on SERIAL_8250_SHARE_IRQ
    on SERIAL_8250_MANY_PORTS
    on SERIAL_8250_DMA
    on HPET_TIMER
    on BINFMT_SCRIPT

    # -- the filesystems a running userland needs to exist at all -------------
    on PROC_FS
    on SYSFS
    on DEVTMPFS
    on DEVTMPFS_MOUNT
    on TMPFS
    on UNIX

    # -- THE POINT: 9p as a ROOT filesystem, built in, not modular ------------
    on NET
    on NET_9P
    on NET_9P_VIRTIO
    on 9P_FS
    on 9P_FS_POSIX_ACL
    on PCI
    # VIRTIO_MENU gates the whole virtio submenu. Without it `--enable
    # VIRTIO_PCI` writes a line that olddefconfig then silently drops, which is
    # precisely the failure the preflight below exists to catch -- and did.
    on VIRTIO_MENU
    on VIRTIO
    on VIRTIO_PCI
    on VIRTIO_PCI_LEGACY
    on VIRTIO_MMIO
    on VIRTIO_CONSOLE

    # -- keep it small --------------------------------------------------------
    on  KERNEL_XZ          # beats the stock GZIP on the one artifact we ship
    off KERNEL_GZIP
    off MODULES            # nothing is modular: there is nothing to load them
    off BLK_DEV_INITRD     # and no initramfs to load them FROM
    off DEBUG_KERNEL
    off KALLSYMS_ALL

    make olddefconfig >/dev/null 2>&1

    # Assert the three that decide whether this boots at all, BEFORE spending
    # ten minutes compiling. olddefconfig silently drops an option whose
    # dependencies are unmet, and a kernel that is merely missing 9p looks
    # exactly like a kernel that is broken when it fails to mount root.
    for sym in CONFIG_9P_FS CONFIG_NET_9P_VIRTIO CONFIG_VIRTIO_PCI CONFIG_SERIAL_8250_CONSOLE CONFIG_X86_IO_APIC CONFIG_SERIAL_8250_PNP; do
      grep -q "^${sym}=y" .config || { echo "FATAL: ${sym} is not builtin"; grep "${sym}" .config || echo "  (absent)"; exit 1; }
    done
    echo "    config ok: 9p, virtio and the serial console are all =y"

    make -j"$(nproc)" bzImage >/dev/null 2>&1
    cp arch/x86/boot/bzImage /out/vmlinuz
    cp .config /out/kernel.config
    echo "    vmlinuz $(stat -c%s /out/vmlinuz) bytes"
    echo "    builtin=$(grep -c "=y$" .config)  modules=$(grep -c "=m$" .config)"
  '

echo "==> ${OUT}/vmlinuz"
