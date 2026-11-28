src = open("/Users/kk/tc-harness-src/transcoder.swift").read()
old = '                audioAsc = sanitizeAudioAsc(rawCookie)\n                print("[asc] sanitized=\\(audioAsc?.count ?? -1)")'
new = '                try? Data(rawCookie).write(to: URL(fileURLWithPath: "/tmp/cookie.bin"))\n                audioAsc = sanitizeAudioAsc(rawCookie)\n                print("[asc] sanitized=\\(audioAsc?.count ?? -1)")'
assert old in src, "anchor missing"
src = src.replace(old, new, 1)
open("/Users/kk/tc-harness-src/transcoder.swift", "w").write(src)
print("patched2")
