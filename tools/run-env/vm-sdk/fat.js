// fat.js -- a 1.44 MB FAT12 floppy, built and read by the page (window.EntityFat).
//
// WHY A FLOPPY: a guest with no 9p (KolibriOS, FreeDOS) still reads FAT floppies,
// and v86 lets the page swap one in and out WHILE THE GUEST RUNS -- measured both
// directions on both guests (DESIGN-2026-09-14 §1.2). A hard disk is not a live
// channel: Kolibri's block cache hides a page-side write until it remounts.
//
// SCOPE, deliberately small: build a fresh image from a flat list of files (a
// rebuild is always defragmented and always consistent, and 1.44 MB costs nothing
// to rewrite), and read every file back including subdirectories and long names.
// No in-place edits: the page never writes into an image a guest may be writing.
//
// Runs in a browser and under node (module.exports), so it is gated headless.
(function (root) {
  'use strict';
  const SECTOR = 512, TOTAL = 2880, SPC = 1, RESERVED = 1, FATS = 2, FAT_SECTORS = 9,
        ROOT_ENTRIES = 224, SPT = 18, HEADS = 2, MEDIA = 0xF0;
  const ROOT_SECTORS = Math.ceil(ROOT_ENTRIES * 32 / SECTOR);                 // 14
  const DATA_START = RESERVED + FATS * FAT_SECTORS + ROOT_SECTORS;             // 33
  const CLUSTERS = TOTAL - DATA_START;                                         // 2847
  const SIZE = TOTAL * SECTOR;                                                 // 1,474,560
  // What a flat root can hold, said up front rather than discovered on a full disk.
  const CAPACITY = CLUSTERS * SECTOR;

  const enc = new TextEncoder();

  // A long name needs LFN entries; a name that is already a valid upper-case 8.3
  // name does not. Lower-case names get LFN entries so `notes.txt` comes back as
  // `notes.txt`, not `NOTES.TXT`.
  const SHORT_OK = /^[A-Z0-9!#$%&'()\-@^_`{}~]{1,8}(\.[A-Z0-9!#$%&'()\-@^_`{}~]{1,3})?$/;

  function shortNameFor(name, taken) {
    if (SHORT_OK.test(name) && !taken.has(name)) return { short: name, lfn: false };
    const dot = name.lastIndexOf('.');
    const clean = s => s.toUpperCase().replace(/[^A-Z0-9!#$%&'()\-@^_`{}~]/g, '');
    let base = clean(dot > 0 ? name.slice(0, dot) : name) || 'FILE';
    const ext = clean(dot > 0 ? name.slice(dot + 1) : '').slice(0, 3);
    for (let n = 1; n < 1000000; n++) {
      const tail = '~' + n;
      const s = base.slice(0, 8 - tail.length) + tail + (ext ? '.' + ext : '');
      if (!taken.has(s)) return { short: s, lfn: true };
    }
    throw new Error('too many similar names');
  }

  function raw83(short) {
    const [b, e = ''] = short.split('.');
    const out = new Uint8Array(11).fill(0x20);
    out.set(enc.encode(b).subarray(0, 8), 0);
    out.set(enc.encode(e).subarray(0, 3), 8);
    return out;
  }

  function lfnChecksum(r) {
    let s = 0;
    for (let i = 0; i < 11; i++) s = (((s & 1) << 7) | (s >> 1)) + r[i] & 0xFF;
    return s;
  }

  function dosTime(ms) {
    const d = new Date(ms);
    const y = Math.min(Math.max(d.getUTCFullYear(), 1980), 2107);
    return {
      date: ((y - 1980) << 9) | ((d.getUTCMonth() + 1) << 5) | d.getUTCDate(),
      time: (d.getUTCHours() << 11) | (d.getUTCMinutes() << 5) | (d.getUTCSeconds() >> 1),
    };
  }

  // Build a fresh floppy holding `files`: [{path | name, data: Uint8Array, mtime?}],
  // plus [{path, dir: true}] for a directory that holds nothing. A path may contain
  // `/` -- directories are created as needed -- so a floppy read with readFloppy
  // and rebuilt from its files keeps its folders (a machine's kept files ARE the
  // floppy's files; the image is rebuilt from them each visit). Throws with a
  // sentence a person can act on when something does not fit.
  function makeFloppy(files, opts) {
    opts = opts || {};
    const img = new Uint8Array(SIZE);
    const dv = new DataView(img.buffer);
    // Boot sector: a BPB and a non-bootable stub that prints nothing and halts.
    img.set([0xEB, 0x3C, 0x90], 0);
    img.set(enc.encode('ENTITYVM'), 3);
    dv.setUint16(11, SECTOR, true); img[13] = SPC; dv.setUint16(14, RESERVED, true);
    img[16] = FATS; dv.setUint16(17, ROOT_ENTRIES, true); dv.setUint16(19, TOTAL, true);
    img[21] = MEDIA; dv.setUint16(22, FAT_SECTORS, true); dv.setUint16(24, SPT, true);
    dv.setUint16(26, HEADS, true);
    img[36] = 0x00; img[38] = 0x29; dv.setUint32(39, opts.serial >>> 0 || 0x20260914, true);
    const label = (opts.label || 'TRANSFER').toUpperCase().slice(0, 11).padEnd(11, ' ');
    img.set(enc.encode(label), 43);
    img.set(enc.encode('FAT12   '), 54);
    img.set([0xFA, 0xF4, 0xEB, 0xFD], 62);                    // cli; hlt; jmp $-1
    img[510] = 0x55; img[511] = 0xAA;

    const fat = new Uint8Array(FAT_SECTORS * SECTOR);
    const setFat = (c, v) => {
      const o = c + (c >> 1);
      if (c & 1) { fat[o] = (fat[o] & 0x0F) | ((v << 4) & 0xF0); fat[o + 1] = (v >> 4) & 0xFF; }
      else { fat[o] = v & 0xFF; fat[o + 1] = (fat[o + 1] & 0xF0) | ((v >> 8) & 0x0F); }
    };
    setFat(0, 0xF00 | MEDIA); setFat(1, 0xFFF);

    // 1. The tree. Names compare case-insensitively, as the guest will.
    const newDir = name => ({ name, dirs: new Map(), files: new Map(), mtime: null });
    const tree = newDir('');
    const badName = n => !n || /[\\\x00-\x1f"*:<>?|]/.test(n) || n === '.' || n === '..';
    for (const f of files) {
      const path = String(f.path != null ? f.path : (f.name || ''));
      const parts = path.split('/');
      for (const part of parts) if (badName(part)) throw new Error(`"${path}" cannot be a file name on a floppy`);
      const leaf = parts.pop();
      let node = tree;
      for (const d of parts) {
        const k = d.toLowerCase();
        if (node.files.has(k)) throw new Error(`${d} is both a file and a folder`);
        if (!node.dirs.has(k)) node.dirs.set(k, newDir(d));
        node = node.dirs.get(k);
      }
      const k = leaf.toLowerCase();
      if (f.dir) {
        if (node.files.has(k)) throw new Error(`${leaf} is both a file and a folder`);
        if (!node.dirs.has(k)) node.dirs.set(k, newDir(leaf));
        continue;
      }
      if (node.files.has(k)) throw new Error(`two files are both called ${leaf}`);
      if (node.dirs.has(k)) throw new Error(`${leaf} is both a file and a folder`);
      const data = f.data instanceof Uint8Array ? f.data : new Uint8Array(f.data || 0);
      node.files.set(k, { name: leaf, data, mtime: f.mtime });
    }

    // 2. Directory records (LFN entries + the short entry) for every child.
    const recordsFor = (name, taken) => {
      const { short, lfn } = shortNameFor(name, taken);
      taken.add(short);
      const r = raw83(short);
      const out = [];
      if (lfn) {
        const units = [];
        for (const ch of name) { const c = ch.codePointAt(0); if (c > 0xFFFF) { const v = c - 0x10000; units.push(0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF)); } else units.push(c); }
        if (units.length > 255) throw new Error(`${name} is longer than 255 characters`);
        const parts = Math.ceil((units.length + 1) / 13);
        const sum = lfnChecksum(r);
        for (let p = parts; p >= 1; p--) {
          const e = new Uint8Array(32);
          e[0] = p | (p === parts ? 0x40 : 0); e[11] = 0x0F; e[13] = sum;
          const at = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
          for (let i = 0; i < 13; i++) {
            const k = (p - 1) * 13 + i;
            const u = k < units.length ? units[k] : k === units.length ? 0 : 0xFFFF;
            e[at[i]] = u & 0xFF; e[at[i] + 1] = u >> 8;
          }
          out.push(e);
        }
      }
      const e = new Uint8Array(32); e.set(r, 0);
      out.push(e);                                               // the short entry: attr, time, cluster, size filled in later
      return out;
    };
    const shortEntry = (e, attr, first, size, mtime) => {
      e[11] = attr;
      const t = dosTime(mtime ? mtime * 1000 : Date.now());
      const edv = new DataView(e.buffer);
      edv.setUint16(14, t.time, true); edv.setUint16(16, t.date, true);
      edv.setUint16(18, t.date, true); edv.setUint16(22, t.time, true); edv.setUint16(24, t.date, true);
      edv.setUint16(26, first, true); edv.setUint32(28, size, true);
    };

    // 3. Lay out: every directory and file gets its clusters, depth first.
    let next = 2;
    const alloc = n => {
      const first = n ? next : 0;
      next += n;
      if (next - 2 > CLUSTERS) throw new Error('the files do not fit on a 1.44 MB floppy');
      return first;
    };
    const writeChain = (first, bytes, n) => {
      for (let i = 0; i < n; i++) {
        const c = first + i;
        setFat(c, i === n - 1 ? 0xFFF : c + 1);
        img.set(bytes.subarray(i * SECTOR, (i + 1) * SECTOR), (DATA_START + c - 2) * SECTOR);
      }
    };
    // How many 32-byte records a directory's children need -- names alone decide it,
    // so a folder's clusters can be allocated before its children are built (their
    // ".." entries must name it).
    const recordCount = node => {
      const taken = new Set();
      let n = 0;
      for (const d of node.dirs.values()) n += recordsFor(d.name, taken).length;
      for (const f of node.files.values()) n += recordsFor(f.name, taken).length;
      return n;
    };
    // Returns the directory's records; clusters below it allocated and written.
    // `self` is this directory's first cluster (0 for the root), `parent` its parent's.
    const build = (node, self) => {
      const taken = new Set();
      const out = [];
      for (const d of node.dirs.values()) {
        const recs = recordsFor(d.name, taken);
        const n = Math.max(1, Math.ceil((2 + recordCount(d)) * 32 / SECTOR));
        const first = alloc(n);
        const inner = build(d, first);
        const buf = new Uint8Array(n * SECTOR);
        const dot = new Uint8Array(32).fill(0x20, 0, 11); dot[0] = 0x2E;
        const dotdot = new Uint8Array(32).fill(0x20, 0, 11); dotdot[0] = 0x2E; dotdot[1] = 0x2E;
        shortEntry(dot, 0x10, first, 0, d.mtime);
        shortEntry(dotdot, 0x10, self, 0, d.mtime);                  // 0 when the parent is the root, per FAT
        buf.set(dot, 0); buf.set(dotdot, 32);
        inner.forEach((rec, k) => buf.set(rec, (2 + k) * 32));
        writeChain(first, buf, n);
        shortEntry(recs[recs.length - 1], 0x10, first, 0, d.mtime);
        out.push(...recs);
      }
      for (const f of node.files.values()) {
        const recs = recordsFor(f.name, taken);
        const n = Math.ceil(f.data.length / SECTOR);
        const first = alloc(n);
        writeChain(first, f.data, n);
        shortEntry(recs[recs.length - 1], 0x20, first, f.data.length, f.mtime);   // archive
        out.push(...recs);
      }
      return out;
    };
    const rootRecs = build(tree, 0);
    if (rootRecs.length + 1 > ROOT_ENTRIES) throw new Error(`a floppy root holds at most ${ROOT_ENTRIES} directory entries`);
    const rootOff = (RESERVED + FATS * FAT_SECTORS) * SECTOR;
    { const e = new Uint8Array(32); e.set(enc.encode(label), 0); e[11] = 0x08; img.set(e, rootOff); }   // volume label
    rootRecs.forEach((rec, i) => img.set(rec, rootOff + (i + 1) * 32));
    for (let i = 0; i < FATS; i++) img.set(fat, (RESERVED + i * FAT_SECTORS) * SECTOR);
    return img;
  }

  // Read every file on a FAT12/16 floppy or superfloppy, subdirectories included.
  // Returns [{path, name, size, data, mtime, dir?}] -- `dir: true` only for an EMPTY
  // folder (a non-empty one is implied by its files) -- or throws on something that is not FAT.
  function readFloppy(u8) {
    const img = u8 instanceof Uint8Array ? u8 : new Uint8Array(u8);
    const dv = new DataView(img.buffer, img.byteOffset, img.byteLength);
    const bps = dv.getUint16(11, true), spc = img[13], rsv = dv.getUint16(14, true);
    const nfats = img[16], nroot = dv.getUint16(17, true), spf = dv.getUint16(22, true);
    const tot = dv.getUint16(19, true) || dv.getUint32(32, true);
    if (![512, 1024, 2048, 4096].includes(bps) || !spc || !nfats || !spf || !nroot) {
      throw new Error('not a FAT12/16 floppy (no usable boot sector)');
    }
    const fat0 = rsv * bps, root0 = fat0 + nfats * spf * bps;
    const data0 = root0 + Math.ceil(nroot * 32 / bps) * bps;
    const clusters = Math.floor((tot - data0 / bps) / spc);
    const fat16 = clusters >= 4085;
    const END = fat16 ? 0xFFF8 : 0xFF8;
    const clusterBytes = spc * bps;
    const nextOf = c => fat16 ? dv.getUint16(fat0 + c * 2, true)
      : ((dv.getUint16(fat0 + c + (c >> 1), true) >> (c & 1 ? 4 : 0)) & 0xFFF);
    const chain = (c, limit) => {
      const out = []; const guard = new Set();
      while (c >= 2 && c < END && !guard.has(c) && (limit == null || out.length * clusterBytes < limit)) {
        guard.add(c);
        const s = data0 + (c - 2) * clusterBytes;
        if (s + clusterBytes > img.length) break;
        out.push(img.subarray(s, s + clusterBytes)); c = nextOf(c);
      }
      const joined = new Uint8Array(out.reduce((a, b) => a + b.length, 0));
      let o = 0; for (const b of out) { joined.set(b, o); o += b.length; }
      return joined;
    };
    const files = [];
    const walk = (dir, prefix, depth) => {
      if (depth > 16) return;
      let lfn = [];
      for (let o = 0; o + 32 <= dir.length; o += 32) {
        const b = dir[o];
        if (b === 0) break;
        if (b === 0xE5) { lfn = []; continue; }
        const attr = dir[o + 11];
        if ((attr & 0x3F) === 0x0F) {
          const at = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
          const units = [];
          for (const i of at) { const u = dir[o + i] | (dir[o + i + 1] << 8); if (u === 0 || u === 0xFFFF) break; units.push(u); }
          lfn.unshift(String.fromCharCode(...units));
          continue;
        }
        if (attr & 0x08) { lfn = []; continue; }                       // volume label
        const base = String.fromCharCode(...dir.subarray(o, o + 8)).trimEnd();
        const ext = String.fromCharCode(...dir.subarray(o + 8, o + 11)).trimEnd();
        const shortName = (b === 0x05 ? '\xE5' + base.slice(1) : base) + (ext ? '.' + ext : '');
        const name = lfn.length ? lfn.join('') : (dir[o + 12] & 0x08 ? shortName.toLowerCase() : shortName);
        lfn = [];
        if (shortName === '.' || shortName === '..') continue;
        const ddv = new DataView(dir.buffer, dir.byteOffset + o, 32);
        const first = ddv.getUint16(26, true), size = ddv.getUint32(28, true);
        const date = ddv.getUint16(24, true), time = ddv.getUint16(22, true);
        const mtime = Date.UTC(1980 + (date >> 9), ((date >> 5) & 15) - 1, date & 31,
                               time >> 11, (time >> 5) & 63, (time & 31) * 2) / 1000;
        const path = prefix + name;
        if (attr & 0x10) {
          const before = files.length;
          walk(chain(first), path + '/', depth + 1);
          // An empty folder is still a thing the person made: report it, so a rebuild keeps it.
          if (files.length === before) files.push({ path, name, dir: true, size: 0, mtime, data: new Uint8Array(0) });
        } else files.push({ path, name, size, mtime, data: chain(first, size).slice(0, size) });
      }
    };
    walk(img.subarray(root0, data0), '', 0);
    return files;
  }

  const api = { makeFloppy, readFloppy, SIZE, CAPACITY, ROOT_ENTRIES };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.EntityFat = api;
})(typeof window !== 'undefined' ? window : this);
