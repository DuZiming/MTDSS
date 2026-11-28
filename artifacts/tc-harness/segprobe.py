import struct, sys

def parse(path):
    data = open(path, 'rb').read()
    def walk(start, end):
        pos = start
        while pos + 8 <= end:
            size = struct.unpack('>I', data[pos:pos+4])[0]
            typ = data[pos+4:pos+8].decode('latin1')
            yield pos, size, typ
            pos += size
    for p, s, t in walk(0, len(data)):
        if t == 'moof':
            for p2, s2, t2 in walk(p+8, p+s):
                if t2 == 'traf':
                    for p3, s3, t3 in walk(p2+8, p2+s2):
                        if t3 == 'trun':
                            b = data[p3+8:p3+s3]
                            flags = struct.unpack('>I', b'\x00'+b[1:4])[0]
                            cnt = struct.unpack('>I', b[4:8])[0]
                            off = struct.unpack('>i', b[8:12])[0]
                            entries = []
                            for i in range(min(cnt, 4)):
                                q = 12 + i*12
                                entries.append(struct.unpack('>III', b[q:q+12]))
                            total = sum(struct.unpack('>I', b[16+i*12:20+i*12])[0] for i in range(cnt))
                            print(f'trun: flags=0x{flags:06x} count={cnt} data_off={off} total_sizes={total}')
                            for i, (dur, sz, fl) in enumerate(entries):
                                print(f'   s[{i}]: dur={dur} size={sz} flags=0x{fl:08x}')
    for p, s, t in walk(0, len(data)):
        if t == 'mdat':
            print(f'mdat @{p} size={s-8}; head: ' + ' '.join(f'{b:02x}' for b in data[p+8:p+28]))

for path in sys.argv[1:]:
    print(f'== {path} ==')
    parse(path)
