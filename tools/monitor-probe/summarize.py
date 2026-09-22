import json, glob, os
C = ["top","worker","srcdoc","urlSame","urlOpaque"]
def short(v, n=90):
    s = v if isinstance(v, str) else json.dumps(v, separators=(",",":"))
    return s[:n]
for f in sorted(glob.glob("results/*.json")):
    r = json.load(open(f)); name = os.path.basename(f)[:-5]
    print(f"\n==================== {name}  ({r['engine']} {r['browserVersion']}) fatal={r.get('fatal')}")
    rows = {
     "isSecureContext": lambda e: e.get("isSecureContext"),
     "crossOriginIsolated": lambda e: e.get("crossOriginIsolated"),
     "origin": lambda e: e.get("origin"),
     "SAB ctor": lambda e: e.get("sabConstruct"),
     "perf.memory": lambda e: e.get("performance_memory"),
     "mUASM typeof": lambda e: e.get("measureUserAgentSpecificMemory"),
     "deviceMemory": lambda e: e.get("deviceMemory"),
     "hwConcurrency": lambda e: e.get("hardwareConcurrency"),
     "storage.estimate": lambda e: e.get("storage_estimate"),
     "persisted": lambda e: e.get("storage_persisted"),
     "connection": lambda e: e.get("connection"),
     "battery": lambda e: e.get("battery", e.get("getBattery")),
     "PressureObserver": lambda e: e.get("PressureObserver"),
     "uaData high": lambda e: e.get("uaData_high", e.get("userAgentData")),
     "now() res 1M min": lambda e: e.get("timer_1M",{}).get("minNonZeroMs"),
     "now() res 10k min": lambda e: e.get("timer_10k",{}).get("minNonZeroMs"),
     "now() distinct": lambda e: e.get("timer_1M",{}).get("smallestDistinctMs"),
     "scheduler.yield": lambda e: e.get("scheduler_yield"),
     "isInputPending": lambda e: e.get("isInputPending"),
     "wasm grow": lambda e: e.get("wasm_memory"),
     "wasm shared mem": lambda e: e.get("wasm_shared_memory"),
     "entryTypes": lambda e: e.get("supportedEntryTypes"),
     "gpu/wakeLock": lambda e: (e.get("navigator_gpu"), e.get("wakeLock")),
    }
    for k, fn in rows.items():
        print(f"  {k:18}", " | ".join(f"{c}={short(fn(r['env'][c]) if isinstance(r['env'].get(c),dict) else r['env'].get(c), 200 if k in ('entryTypes',) else 70)}" for c in C))
    print("  -- busy windows (top rAF maxGap / top 10ms-timeout maxGap / ping rtt / top-visible entries)")
    for w in r["busy"]:
        ents = [(x["entryType"], x.get("name"), x["duration"], [(a["containerType"],a["containerId"],a["containerSrc"]) for a in x.get("attribution",[])], [(s["invoker"],s["sourceURL"],s["windowAttribution"],s["duration"]) for s in x.get("scripts",[])]) for x in w["topEntriesInWindow"]]
        print(f"    {w['who']:14} rAF={w['topRaf']['maxGapMs']:6} t10={w['topTimeout10ms']['maxGapMs']:6} ping={w.get('ping',{}).get('rttMs')}  entries={ents}")
    print("  frame own entries (count by type):", {k:(v if isinstance(v,str) else {t:sum(1 for x in v if x['entryType']==t) for t in ('longtask','long-animation-frame')}) for k,v in r["frameOwnEntries"].items()})
    print("  topObserve:", r["topObserve"], " topEntriesAll:", len(r["topEntriesAll"]))
    print("  -- resource timing, each ctx's OWN entries: (name, transfer, encoded)")
    for c in C:
        fe = r["fetch"].get(c)
        if isinstance(fe, dict): print(f"    {c:10}", [(x["name"].split("/")[-1], x["transferSize"], x["encodedBodySize"], x["nextHopProtocol"]) for x in fe.get("ownEntries",[])])
        else: print(f"    {c:10}", fe)
    print("    TOP sees:", [(x["name"].split("/")[-1], x["initiatorType"], x["transferSize"]) for x in r["topResourceEntries"]])
    print("  frameWorkers:", short(r["frameWorkers"], 600))
    print("  -- pressure");
    for c in C: print(f"    {c:10}", short(r["pressure"].get(c), 300))
    if "pressureStress" in r: print("    STRESS", short(r["pressureStress"], 1200))
    print("  -- memory")
    for c in C: print(f"    {c:10}", short(r["memory"].get(c), 1500))
