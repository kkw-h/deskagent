"""LNK 结构解析验证：正确实现 LinkInfo + StringData 读取。

用来确认"27/34 是解析器上限还是元数据上限"。
"""
import struct
import sys
from pathlib import Path

DESKTOP = Path.home() / "Desktop"


def parse_lnk(path: Path) -> dict:
    b = path.read_bytes()
    if len(b) < 76 or b[:4] != b"L\x00\x00\x00":
        return {"target": "", "relpath": "", "workdir": "", "err": "bad header"}

    flags = struct.unpack_from("<I", b, 0x14)[0]
    off = 76
    target = ""

    # --- LinkTargetIDList ---
    if flags & 0x01:
        size = struct.unpack_from("<H", b, off)[0]
        off += 2 + size

    # --- LinkInfo ---
    if flags & 0x02 and off + 28 <= len(b):
        li_size = struct.unpack_from("<I", b, off)[0]
        li = b[off : off + li_size]
        if len(li) >= 28:
            hdr = struct.unpack_from("<I", li, 4)[0]
            lbp = struct.unpack_from("<I", li, 16)[0]
            cps = struct.unpack_from("<I", li, 24)[0]

            base = _ansi(li, lbp)
            suffix = _ansi(li, cps)
            target = base + suffix

            if hdr >= 0x24 and len(li) >= 36:
                lbpu = struct.unpack_from("<I", li, 28)[0]
                cpsu = struct.unpack_from("<I", li, 32)[0]
                ubase = _wide(li, lbpu)
                usuffix = _wide(li, cpsu)
                if ubase:
                    target = ubase + usuffix
        off += li_size

    # --- StringData ---
    name = relpath = workdir = args = ""
    for bit, key in ((0x04, "name"), (0x08, "relpath"), (0x10, "workdir"), (0x20, "args")):
        if not flags & bit:
            continue
        if off + 2 > len(b):
            break
        n = struct.unpack_from("<H", b, off)[0]
        s = b[off + 2 : off + 2 + n * 2].decode("utf-16-le", errors="ignore")
        off += 2 + n * 2
        if key == "name":
            name = s
        elif key == "relpath":
            relpath = s
        elif key == "workdir":
            workdir = s
        elif key == "args":
            args = s

    return {"target": target, "relpath": relpath, "workdir": workdir, "args": args, "err": ""}


def _ansi(li: bytes, off: int) -> str:
    if not off or off >= len(li):
        return ""
    raw = li[off:].split(b"\x00", 1)[0]
    for enc in ("gbk", "latin-1"):
        try:
            return raw.decode(enc)
        except Exception:
            continue
    return ""


def _wide(li: bytes, off: int) -> str:
    if not off or off + 1 >= len(li):
        return ""
    try:
        return li[off:].decode("utf-16-le", errors="ignore").split("\x00", 1)[0]
    except Exception:
        return ""


def main() -> int:
    rows = []
    for f in sorted(DESKTOP.iterdir()):
        if f.is_file() and f.suffix.lower() == ".lnk":
            r = parse_lnk(f)
            t = r["target"]
            if not t and r["workdir"] and r["relpath"]:
                t = str(Path(r["workdir"]) / r["relpath"])
            rows.append((f.stem, t, r["relpath"], r["workdir"]))

    total = len(rows)
    ok = sum(1 for r in rows if r[1])
    print(f"TOTAL={total}  HAS_TARGET={ok}  RATE={ok * 100 // total}%")
    print("-" * 70)
    for name, t, rel, wd in rows:
        if t:
            print(f"  {name:<22} {t}")
        else:
            print(f"! {name:<22} rel={rel!r} wd={wd!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
