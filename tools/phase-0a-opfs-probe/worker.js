// Phase 0a — OPFS probe, worker side.
// Runs the OPFS SyncAccessHandle smoke test inside a dedicated Worker
// and reports each step back to the main thread for tabulation.

self.postMessage({ type: 'ready' });

self.onmessage = async (e) => {
  if (e.data?.type !== 'run-opfs-probe') return;
  const results = {};

  const record = (id, ok, detail) => {
    results[id] = { ok, detail };
  };

  // Test 3: navigator.storage.getDirectory()
  let root;
  try {
    if (!self.navigator?.storage?.getDirectory) {
      throw new Error('navigator.storage.getDirectory is undefined in worker');
    }
    root = await navigator.storage.getDirectory();
    record('getdir', true, 'Got OPFS root directory handle.');
  } catch (err) {
    record('getdir', false, `${err.name}: ${err.message}`);
    // Without the root we can't do steps 4-7.
    ['fh', 'sah', 'rw', 'cleanup'].forEach(k =>
      record(k, false, 'Skipped — no OPFS root.'));
    self.postMessage({ type: 'result', results });
    return;
  }

  // Test 4: getFileHandle with create:true.
  let fileHandle;
  try {
    fileHandle = await root.getFileHandle('phase-0a-probe.bin', { create: true });
    record('fh', true, 'File handle obtained.');
  } catch (err) {
    record('fh', false, `${err.name}: ${err.message}`);
    ['sah', 'rw', 'cleanup'].forEach(k =>
      record(k, false, 'Skipped — no file handle.'));
    self.postMessage({ type: 'result', results });
    return;
  }

  // Test 5: createSyncAccessHandle.
  // This is the critical capability — OPFS sync I/O is the whole point.
  let sah;
  try {
    if (typeof fileHandle.createSyncAccessHandle !== 'function') {
      throw new Error('createSyncAccessHandle is not a function on file handle');
    }
    sah = await fileHandle.createSyncAccessHandle();
    record('sah', true, 'SyncAccessHandle obtained.');
  } catch (err) {
    record('sah', false, `${err.name}: ${err.message}`);
    ['rw', 'cleanup'].forEach(k =>
      record(k, false, 'Skipped — no sync handle.'));
    self.postMessage({ type: 'result', results });
    return;
  }

  // Test 6: write + read 4KB via the sync handle.
  try {
    const payload = new Uint8Array(4096);
    for (let i = 0; i < payload.length; i++) payload[i] = (i * 31 + 7) & 0xff;

    const written = sah.write(payload, { at: 0 });
    if (written !== payload.length) {
      throw new Error(`write returned ${written}, expected ${payload.length}`);
    }

    const readback = new Uint8Array(4096);
    const read = sah.read(readback, { at: 0 });
    if (read !== payload.length) {
      throw new Error(`read returned ${read}, expected ${payload.length}`);
    }
    for (let i = 0; i < payload.length; i++) {
      if (readback[i] !== payload[i]) {
        throw new Error(`mismatch at byte ${i}: got ${readback[i]}, expected ${payload[i]}`);
      }
    }
    record('rw', true, 'Wrote and read back 4KB; bytes match.');
  } catch (err) {
    record('rw', false, `${err.name || 'Error'}: ${err.message}`);
  }

  // Test 7: close + cleanup.
  try {
    sah.close();
    await root.removeEntry('phase-0a-probe.bin');
    record('cleanup', true, 'Closed sync handle and removed probe file.');
  } catch (err) {
    record('cleanup', false, `${err.name || 'Error'}: ${err.message}`);
  }

  self.postMessage({ type: 'result', results });
};
