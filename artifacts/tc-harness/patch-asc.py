import re

src = open("/Users/kk/tc-harness-src/transcoder.swift").read()
old = "                audioAsc = sanitizeAudioAsc(audioAscSeedFor(passthrough: true, sourceCsd: sourceAsc) ?? [])"
assert old in src, "anchor missing"
new = (
    "                let rawCookie = audioAscSeedFor(passthrough: true, sourceCsd: sourceAsc) ?? []\n"
    '                print("[asc] raw=\\(rawCookie.count)B hex=\\(rawCookie.reduce(into: "") { acc, b in acc += String(b, radix: 16) })")\n'
    "                audioAsc = sanitizeAudioAsc(rawCookie)\n"
    '                print("[asc] sanitized=\\(audioAsc?.count ?? -1)")'
)
src = src.replace(old, new, 1)
open("/Users/kk/tc-harness-src/transcoder.swift", "w").write(src)
print("patched")
