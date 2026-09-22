;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;;                                                                            ;;
;; VMMOUSE.SYS -- absolute pointer for KolibriOS through the VMware mouse     ;;
;; interface (I/O port 0x5658), as emulated by v86, QEMU (-device vmmouse)    ;;
;; and VMware.                                                                ;;
;;                                                                            ;;
;; SPDX-License-Identifier: GPL-2.0-only                                      ;;
;; It includes KolibriOS's driver headers, which are GPL-2.0, so it is too.   ;;
;;                                                                            ;;
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
;
; Why it exists: a PS/2 mouse is RELATIVE. A page showing the machine in a
; browser has the host's pointer and the guest's pointer, moving at different
; rates (KolibriOS accelerates PS/2 movement; the page scales the screen), so
; they drift apart and nothing lines them up again without pointer lock. The
; VMware interface carries the host pointer's ABSOLUTE position, 0..65535 on
; each axis, and KolibriOS's kernel already accepts absolute coordinates from a
; driver (set_mouse_data, bits 31/30 of the button word, 0..32767 per axis).
; This driver is the bridge: nothing more.
;
; Protocol (the same sequence Linux's vmmouse driver uses):
;   eax = 0x564D5868 ('VMXh'), ebx = argument, ecx = command, dx = 0x5658; in eax, dx
;   GETVERSION (10)  -> ebx = magic when the interface is there
;   COMMAND (41), ebx = READ_ID           -> queues the version id
;   STATUS (40)      -> ax = words queued (0xFFFF = error)
;   DATA (39), ebx = n -> eax, ebx, ecx, edx = the next n words
;   COMMAND (41), ebx = REQUEST_ABSOLUTE  -> packets carry absolute positions
; A packet is four words: status (buttons 0x20 left, 0x10 right, 0x08 middle;
; 0x10000 = relative packet), x, y, z (signed wheel).
;
; It polls from a kernel timer (100 Hz) rather than hooking IRQ 12: the PS/2
; IRQ belongs to the kernel's own PS/2 code, and the host silences that path
; while this driver is in absolute mode.

format PE DLL native 0.05
entry START

section '.flat' code readable writable executable
data fixups
end data

include 'proc32.inc'
include 'struct.inc'
include 'macros.inc'
include 'peimport.inc'

VM_MAGIC        = 0x564D5868
VM_PORT         = 0x5658
VM_GETVERSION   = 10
VM_DATA         = 39
VM_STATUS       = 40
VM_COMMAND      = 41
VM_READ_ID      = 0x45414552
VM_DISABLE      = 0x000000F5
VM_REQ_ABSOLUTE = 0x53424152
VM_VERSION_ID   = 0x3442554A

PKT_LEFT        = 0x20
PKT_RIGHT       = 0x10
PKT_MIDDLE      = 0x08
PKT_RELATIVE    = 0x00010000

; eax <- magic, ebx <- arg, ecx <- cmd, then IN. Clobbers eax ebx ecx edx.
macro vmcall cmd, arg
{
        mov     eax, VM_MAGIC
        mov     ebx, arg
        mov     ecx, cmd
        mov     edx, VM_PORT
        in      eax, dx
}

proc START c, state:dword, cmdline:dword
        push    ebx
        cmp     [state], DRV_ENTRY
        je      .init
        cmp     [state], DRV_EXIT
        je      .fini
        jmp     .fail
  .init:
        vmcall  VM_GETVERSION, not VM_MAGIC
        cmp     ebx, VM_MAGIC
        jne     .fail                   ; no VMware interface on this machine
        vmcall  VM_COMMAND, VM_READ_ID
        vmcall  VM_STATUS, 0
        and     eax, 0xFFFF
        cmp     eax, 0xFFFF
        je      .fail
        test    eax, eax
        jz      .fail
        vmcall  VM_DATA, 1
        cmp     eax, VM_VERSION_ID
        jne     .fail
        vmcall  VM_COMMAND, VM_REQ_ABSOLUTE
        invoke  TimerHS, 0, 1, poll, 0
        test    eax, eax
        jz      .disable
        mov     [timer], eax
        invoke  RegService, my_service, service_proc
        pop     ebx
        ret
  .disable:
        vmcall  VM_COMMAND, VM_DISABLE
  .fail:
        xor     eax, eax
        pop     ebx
        ret
  .fini:
        mov     eax, [timer]
        test    eax, eax
        jz      @f
        invoke  CancelTimerHS, eax
        mov     [timer], 0
    @@:
        vmcall  VM_COMMAND, VM_DISABLE
        xor     eax, eax
        pop     ebx
        ret
endp

; Called by the kernel's timer list with one argument (userData): stdcall.
proc poll stdcall uses ebx esi edi, userData:dword
  .next:
        vmcall  VM_STATUS, 0
        and     eax, 0xFFFF
        cmp     eax, 0xFFFF
        je      .done                   ; interface error: stay quiet, the host resets it
        cmp     eax, 4
        jb      .done
        vmcall  VM_DATA, 4              ; eax status, ebx x, ecx y, edx z
        xor     esi, esi
        test    eax, PKT_LEFT
        jz      @f
        or      esi, 1
    @@:
        test    eax, PKT_RIGHT
        jz      @f
        or      esi, 2
    @@:
        test    eax, PKT_MIDDLE
        jz      @f
        or      esi, 4
    @@:
        movsx   edi, dl                 ; wheel, signed
        test    eax, PKT_RELATIVE
        jnz     .relative
        or      esi, 0xC0000000         ; absolute X and Y
        shr     ebx, 1                  ; 0..65535 -> 0..32767, the kernel's range
        shr     ecx, 1
        invoke  SetMouseData, esi, ebx, ecx, edi, 0
        jmp     .next
  .relative:
        invoke  SetMouseData, esi, ebx, ecx, edi, 0
        jmp     .next
  .done:
        ret
endp

proc service_proc stdcall, ioctl:dword
        mov     edx, [ioctl]
        mov     eax, [edx + IOCTL.io_code]
        test    eax, eax                ; SRV_GETVERSION
        jnz     .fail
        cmp     [edx + IOCTL.out_size], 4
        jne     .fail
        mov     eax, [edx + IOCTL.output]
        mov     dword [eax], 1
        xor     eax, eax
        ret
  .fail:
        or      eax, -1
        ret
endp

timer           dd 0
my_service      db 'VMMOUSE', 0
