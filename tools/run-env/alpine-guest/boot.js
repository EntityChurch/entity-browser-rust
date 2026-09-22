#!/usr/bin/env node
// boot.js -- boot the Alpine guest under v86 HEADLESSLY and report what happened.
//
// v86 runs in node (its loader falls back to fs.promises when XMLHttpRequest is
// absent), so the guest can be booted with no browser, no Selenium grid and no
// display. That is what makes this a gate you can run in a loop rather than a
// thing you eyeball.
//
//   node boot.js [--timeout 90] [--cmd 'echo hi']
//
// Exit 0 only if a shell prompt was reached AND every assertion passed.
"use strict";

const fs = require("fs");
const path = require("path");

const args = process.argv.slice(2);
const opt = (name, dflt) => {
  const i = args.indexOf("--" + name);
  return i >= 0 ? args[i + 1] : dflt;
};
const TIMEOUT_S = parseInt(opt("timeout", "90"), 10);
// --cmd runs an extra command at the prompt and prints what the guest said. The
// usage line promised this before it existed, which is the exact coupling AP37
// is about: a documented invocation nobody ran.
const EXTRA_CMD = opt("cmd", null);
const HERE = __dirname;

global.window = undefined;              // v86 checks for a DOM; make sure it finds none
const { V86 } = require(path.join(HERE, "build/libv86.js"));

let serial = "";
const ANSI = /\x1b\[[0-9;?]*[a-zA-Z]/g;
const plain = () => serial.replace(ANSI, "");
let lastOut = Date.now();
const say = (s) => process.stdout.write(s);

// Count what the guest FAULTS IN. The whole claim of the image format is that
// nothing is fetched because it is in the image, only because the guest read it
// -- so the interesting number is not the image size, it is this.
let blobReads = 0, promptAt = 0;
const blobSeen = new Map();
const blobDir = path.join(HERE, "guest/blobs");
const realRead = fs.readFileSync;

const emulator = new V86({
  wasm_path: path.join(HERE, "build/v86.wasm"),
  memory_size: 256 * 1024 * 1024,
  vga_memory_size: 2 * 1024 * 1024,
  bios: { url: path.join(HERE, "bios/seabios.bin") },
  vga_bios: { url: path.join(HERE, "bios/vgabios.bin") },
  bzimage: { url: path.join(HERE, "guest/vmlinuz") },
  initrd: { url: path.join(HERE, "guest/initramfs") },
  // root=host9p is v86's 9p mount tag -- read out of the engine, not guessed:
  // kd()'s configspace_tagname is [104,111,115,116,57,112] = "host9p".
  cmdline: "rw root=host9p rootfstype=9p rootflags=trans=virtio,cache=loose " +
           "console=ttyS0 console=tty0 init=/sbin/init",
  filesystem: {
    basefs: { url: path.join(HERE, "guest/fs.json") },
    baseurl: blobDir + "/",
  },
  autostart: true,
  disable_speaker: true,
});

emulator.add_listener("serial0-output-byte", (byte) => {
  const ch = String.fromCharCode(byte);
  serial += ch;
  lastOut = Date.now();
  say(ch);
});

// --- the assertions, each one a thing that can independently be wrong --------
const CHECKS = [
  ["kernel started",   () => /Linux version/i.test(serial)],
  ["initramfs ran",    () => /Mounting root|Alpine Init|initramfs/i.test(serial)],
  ["9p root mounted",  () => !/Unable to mount root|VFS: Cannot open root/i.test(serial)],
  ["userland reached", () => /\$ $|# $|login: $|Welcome to Alpine/im.test(plain().trimEnd() + (/[$#] $/.test(plain()) ? "" : ""))
                              || /[$#] $/.test(plain())],
];

const started = Date.now();
const probeAnswered = () => /PROBE-Linux i\d86-\d+-OK/.test(plain());
let sentProbe = false;

const tick = setInterval(() => {
  const elapsed = (Date.now() - started) / 1000;

  // Once we see a prompt, prove the guest is ALIVE rather than merely noisy:
  // ask it something only a working userland can answer.
  if (!sentProbe && /[$#] $|login: $/.test(plain().slice(-16))) {
    sentProbe = true; promptAt = Date.now();
    setTimeout(() => {
      emulator.serial0_send(
        "echo PROBE-$(uname -sm)-$(/bin/bash -c 'echo ${BASH_VERSION%%.*}')-OK\n");
      if (EXTRA_CMD) setTimeout(() => emulator.serial0_send(EXTRA_CMD + "\n"), 1500);
    }, 500);
  }

  if (probeAnswered() && !EXTRA_CMD) { clearInterval(tick); finish("probe answered"); }
  else if (elapsed > TIMEOUT_S) { clearInterval(tick); finish("timeout"); }
}, 250);

function finish(reason) {
  try { emulator.stop(); } catch (e) {}
  const secs = ((Date.now() - started) / 1000).toFixed(1);
  say("\n\n" + "=".repeat(68) + "\n");
  say(`BOOT REPORT   (${reason}, ${secs}s, ${serial.length} bytes of serial)\n`);
  say("=".repeat(68) + "\n");

  let failed = 0;
  for (const [name, fn] of CHECKS) {
    let ok = false;
    try { ok = fn(); } catch (e) {}
    if (!ok) failed++;
    say(`  ${ok ? "PASS" : "FAIL"}  ${name}\n`);
  }

  const probed = probeAnswered();
  say(`  ${probed ? "PASS" : "FAIL"}  guest answered a command (bash + uname)\n`);
  if (!probed) failed++;

  const mib = (n) => (n / (1 << 20)).toFixed(1) + " MiB";
  say(`\n  time to prompt:   ${promptAt ? ((promptAt - started) / 1000).toFixed(1) + "s" : "never"}\n`);
  let uniqBytes = 0; for (const v of blobSeen.values()) uniqBytes += v;
  say(`  faulted in:       ${blobSeen.size} distinct blobs, ${mib(uniqBytes)}` +
      ` (${blobReads} reads)\n`);
  say(`                    The image is LAZY, so THIS is the transfer a boot costs --\n`);
  say(`                    not the image size. Distinct blobs, never read events.\n`);
  say(`\n${failed === 0 ? "*** GUEST IS UP ***" : `*** ${failed} CHECK(S) FAILED ***`}\n`);
  process.exit(failed === 0 ? 0 : 1);
}

// count lazy faults by watching the blob directory get read
fs.readFileSync = function (p, ...rest) {
  if (typeof p === "string" && p.startsWith(blobDir)) { blobReads++; if (!blobSeen.has(p)) { try { blobSeen.set(p, fs.statSync(p).size); } catch (e) { blobSeen.set(p, 0); } } }
  return realRead.call(fs, p, ...rest);
};
const realRF = fs.promises.readFile;
fs.promises.readFile = function (p, ...rest) {
  if (typeof p === "string" && p.startsWith(blobDir)) { blobReads++; if (!blobSeen.has(p)) { try { blobSeen.set(p, require("fs").statSync(p).size); } catch (e) { blobSeen.set(p, 0); } } }
  return realRF.call(fs.promises, p, ...rest);
};

process.on("SIGINT", () => finish("interrupted"));
setTimeout(() => finish("deadline"), (TIMEOUT_S + 5) * 1000);
