# Android doc 白屏复现台:r70d 真实资产 + 仿引擎回环(跨源/Range/preflight/CORS)
# 形状对齐 Android:previewer 页在一个 origin,raw 在 127.0.0.1 另一 origin
# (等价 appassets→loopback 跨源面;桌面 Chrome 对 localhost 混合内容豁免,
# 故两侧都用 http,保跨源+preflight 语义)。
import http.server
import socketserver
import io
import re

ASSETS = r"C:\Users\Administrator\Desktop\MTDSS\.claude\worktrees\v4p1\android\app\src\main\assets"

# --- 极小合法 PDF(1 页,经典 xref;供 pdf.js 解析) ---
def make_pdf() -> bytes:
    objs = []
    objs.append(b"<< /Type /Catalog /Pages 2 0 R >>")
    objs.append(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
    objs.append(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>")
    stream = b"BT /F1 24 Tf 20 40 Td (MTDSS doc probe) Tj ET"
    objs.append(b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"\nendstream")
    objs.append(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    out = io.BytesIO()
    out.write(b"%PDF-1.4\n")
    offs = []
    for i, o in enumerate(objs, 1):
        offs.append(out.tell())
        out.write(str(i).encode() + b" 0 obj\n" + o + b"\nendobj\n")
    xref = out.tell()
    out.write(b"xref\n0 " + str(len(objs) + 1).encode() + b"\n")
    out.write(b"0000000000 65535 f \n")
    for off in offs:
        out.write(("%010d 00000 n \n" % off).encode())
    out.write(b"trailer\n<< /Size " + str(len(objs) + 1).encode() + b" /Root 1 0 R >>\nstartxref\n" + str(xref).encode() + b"\n%%EOF")
    return out.getvalue()

PDF = make_pdf()
TEXT = ("MTDSS doc probe text file\n" + "line %d lorem ipsum\n" * 200).encode()

FILES = {"h-pdf": PDF, "h-txt": TEXT}
TOKEN = "tok123"

def cors_headers(extra=None):
    h = {
        "Access-Control-Allow-Origin": "*",
        "Access-Control-Allow-Methods": "GET, OPTIONS",
        "Access-Control-Allow-Headers": "*",
        "Access-Control-Max-Age": "86400",
        "Access-Control-Expose-Headers": "Content-Range, Content-Length",
    }
    if extra:
        h.update(extra)
    return h

class RawHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        print("[raw] " + fmt % args, flush=True)

    def do_OPTIONS(self):
        self.send_response(204)
        for k, v in cors_headers().items():
            self.send_header(k, v)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self):
        m = re.match(r"^/f/([a-z0-9-]+)/raw$", self.path.split("?")[0])
        q = self.path.split("?")[-1]
        if "t=" + TOKEN not in q:
            self.send_response(403); self.send_header("Content-Length", "0"); self.end_headers(); return
        if not m or m.group(1) not in FILES:
            self.send_response(404); self.send_header("Content-Length", "0"); self.end_headers(); return
        data = FILES[m.group(1)]
        rng = self.headers.get("Range")
        if rng:
            mm = re.match(r"bytes=(\d+)-(\d*)", rng)
            begin = int(mm.group(1))
            end = int(mm.group(2)) if mm.group(2) else len(data) - 1
            end = min(end, len(data) - 1)
            chunk = data[begin:end + 1]
            self.send_response(206)
            for k, v in cors_headers({"Content-Range": "bytes %d-%d/%d" % (begin, end, len(data))}).items():
                self.send_header(k, v)
            self.send_header("Content-Type", "application/pdf" if m.group(1) == "h-pdf" else "text/plain")
            self.send_header("Content-Length", str(len(chunk)))
            self.end_headers()
            self.wfile.write(chunk)
        else:
            self.send_response(200)
            for k, v in cors_headers().items():
                self.send_header(k, v)
            self.send_header("Content-Type", "application/pdf" if m.group(1) == "h-pdf" else "text/plain")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

class AssetHandler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=ASSETS, **kw)

    def log_message(self, fmt, *args):
        print("[assets] " + fmt % args, flush=True)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

if __name__ == "__main__":
    import threading
    t1 = socketserver.ThreadingTCPServer(("127.0.0.1", 8792), RawHandler)
    t2 = socketserver.ThreadingTCPServer(("127.0.0.1", 8791), AssetHandler)
    threading.Thread(target=t1.serve_forever, daemon=True).start()
    print("raw loopback on 8792, assets on 8791", flush=True)
    t2.serve_forever()
