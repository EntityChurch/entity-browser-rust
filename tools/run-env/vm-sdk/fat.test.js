// fat.test.js -- node gate for fat.js, and a cross-check against mtools when the
// caller hands us its output. Run through a container (no node on the host):
//
//   podman run --rm --security-opt label=disable -v "$PWD:/w" -w /w \
//     docker.io/library/node:20-slim node vm-sdk/fat.test.js [written.img mtools-listing.txt]
//
// Exit 0 iff every check passes.
'use strict';
const fs = require('fs');
const { makeFloppy, readFloppy, SIZE, CAPACITY } = require('./fat.js');
let fail = 0;
const ok = (cond, what, got) => { console.log(`${cond ? '  PASS' : '  FAIL'}  ${what}${cond ? '' : '  got ' + JSON.stringify(got)}`); if (!cond) fail++; };
const bytes = s => new TextEncoder().encode(s);
const eq = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);

// 1. round trip: short names, a long lower-case name, an empty file, a multi-cluster file
const big = new Uint8Array(70000).map((_, i) => (i * 131) & 255);
const files = [
  { name: 'README.TXT', data: bytes('upper 8.3\r\n') },
  { name: 'notes from the host.md', data: bytes('# a long name with spaces\n') },
  { name: 'empty.txt', data: new Uint8Array(0) },
  { name: 'big.bin', data: big },
  { name: 'Café-ñ.txt', data: bytes('non-ascii name') },
];
const img = makeFloppy(files);
ok(img.length === SIZE, 'the image is exactly 1,474,560 bytes', img.length);
const back = readFloppy(img);
ok(back.length === files.length, 'every file comes back', back.map(f => f.path));
for (const f of files) {
  const g = back.find(b => b.name === f.name);
  ok(!!g && eq(g.data, f.data), `${f.name} comes back byte-identical under its own name`, g && { name: g.name, size: g.size });
}

// 2. the refusals a person can act on
const refuses = (fn, needle, what) => { try { fn(); ok(false, what, 'no error'); } catch (e) { ok(String(e).includes(needle), what, String(e)); } };
refuses(() => makeFloppy([{ name: 'x', data: new Uint8Array(CAPACITY + 1) }]), 'do not fit', 'more than a floppy holds is refused, with the reason');
refuses(() => makeFloppy([{ name: 'a\\b', data: bytes('x') }]), 'cannot be a file name', 'a backslash is refused');
refuses(() => makeFloppy([{ path: 'x', data: bytes('1') }, { path: 'x/y', data: bytes('2') }]), 'both a file and a folder', 'a name used as a file and a folder is refused');
refuses(() => makeFloppy([{ name: 'A.TXT', data: bytes('1') }, { name: 'a.txt', data: bytes('2') }]), 'two files', 'names that collide on a case-insensitive disk are refused');
refuses(() => readFloppy(new Uint8Array(SIZE)), 'not a FAT', 'an all-zero image is not read as an empty disk');

// 2b. folders: a floppy read back and rebuilt from its files keeps its folders
// (the kept unit is the files; the image is rebuilt each visit)
const tree = [
  { path: 'top.txt', data: bytes('top') },
  { path: 'docs/Report 2026.md', data: bytes('# nested, long name') },
  { path: 'docs/deeper/still/NOTE.TXT', data: bytes('three levels down') },
  { path: 'docs/deeper/big.bin', data: big },
  { path: 'empty folder', dir: true },
];
// Enough entries that a subdirectory needs more than one cluster (16 records per 512 bytes).
for (let i = 0; i < 40; i++) tree.push({ path: `many/file number ${i}.txt`, data: bytes('n' + i) });
const timg = makeFloppy(tree);
const tback = readFloppy(timg);
for (const f of tree) {
  const g = tback.find(b => b.path === f.path);
  ok(!!g && (f.dir ? g.dir === true : eq(g.data, f.data)), `${f.path} comes back at its path`, g && { path: g.path, dir: g.dir, size: g.size });
}
ok(tback.length === tree.length, 'nothing extra comes back', tback.map(f => f.path));
const again = readFloppy(makeFloppy(tback));
ok(again.length === tback.length && again.every((f, i) => f.path === tback[i].path && eq(f.data, tback[i].data)),
   'read -> rebuild -> read is stable', again.map(f => f.path));
if (process.argv[2] === '--write-tree') { fs.writeFileSync(process.argv[3], timg); console.log('  wrote', process.argv[3]); }

// 3. an empty floppy is empty, not an error
ok(readFloppy(makeFloppy([])).length === 0, 'an empty floppy lists no files');

// 4. exactly full fits
ok(readFloppy(makeFloppy([{ name: 'FULL.BIN', data: new Uint8Array(CAPACITY) }]))[0].size === CAPACITY, 'a file of exactly the capacity fits');

// 5. cross-check with mtools (optional): the harness writes our image, mtools reads it,
// mtools writes one, we read it.
if (process.argv[2] === '--write') {
  fs.writeFileSync(process.argv[3], makeFloppy(files));
  console.log('  wrote', process.argv[3]);
} else if (process.argv[2] === '--read') {
  const got = readFloppy(fs.readFileSync(process.argv[3]));
  const want = JSON.parse(process.argv[4]);
  for (const [path, text] of Object.entries(want)) {
    const g = got.find(f => f.path === path);
    ok(!!g && Buffer.from(g.data).toString() === text, `mtools-written ${path} reads back`, got.map(f => f.path));
  }
}
console.log(fail ? `FAILED ${fail}` : 'all fat.js checks pass');
process.exit(fail ? 1 : 0);
