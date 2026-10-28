"""M0 元数据探针：验证 .lnk / .url 能挖出多少可用于分类的信息。

不依赖 COM，纯二进制解析。这个脚本是 crates/scan 的原型。
"""
import re
import sys
from pathlib import Path

DESKTOP = Path.home() / "Desktop"

PATH_RE = re.compile(
    r'[A-Za-z]:\\[^\\/:*?"<>|\x00-\x1f]{1,}'
    r'(?:\\[^\\/:*?"<>|\x00-\x1f]{1,})*'
    r'\.(?:exe|com|bat|cmd)',
    re.I,
)


def lnk_target(p: Path) -> str:
    b = p.read_bytes()
    for enc in ("utf-16-le", "gbk", "latin-1"):
        try:
            t = b.decode(enc, errors="ignore")
        except Exception:
            continue
        hits = PATH_RE.findall(t)
        if hits:
            return max(hits, key=len)
    return ""


def url_target(p: Path) -> str:
    try:
        for line in p.read_text(errors="ignore").splitlines():
            if line.upper().startswith("URL="):
                return line[4:].strip()
    except Exception:
        pass
    return ""


def guess_vendor(target: str) -> str:
    """从目标路径猜发行商，模拟 PE 版本信息的粗替身。"""
    t = target.lower()
    table = {
        "steam": "Valve",
        "epic": "Epic Games",
        "wegame": "腾讯",
        "tencent": "腾讯",
        "netease": "网易",
        "google\\chrome": "Google",
        "microsoft\\edge": "Microsoft",
        "nvidia": "NVIDIA",
        "obs-studio": "OBS",
        "vmware": "VMware",
        "telegram": "Telegram",
        "feishu": "字节跳动",
        "doubao": "字节跳动",
        "bytedance": "字节跳动",
        "wps": "金山",
        "kingsoft": "金山",
        "quark": "阿里",
        "bilibili": "哔哩哔哩",
        "cloudmusic": "网易",
        "clash": "Clash",
        "todesk": "ToDesk",
        "quicklook": "QuickLook",
        "zed": "Zed",
        "yaak": "Yaak",
    }
    for k, v in table.items():
        if k in t:
            return v
    return ""


def main() -> int:
    if not DESKTOP.exists():
        print(f"桌面不存在: {DESKTOP}", file=sys.stderr)
        return 1

    rows = []
    for f in sorted(DESKTOP.iterdir()):
        if not f.is_file():
            continue
        ext = f.suffix.lower()
        if ext == ".lnk":
            t = lnk_target(f)
        elif ext == ".url":
            t = url_target(f)
        else:
            continue
        rows.append((f.stem, ext, t, guess_vendor(t)))

    total = len(rows)
    has_target = sum(1 for r in rows if r[2])
    has_vendor = sum(1 for r in rows if r[3])

    print(f"TOTAL={total}  HAS_TARGET={has_target}  HAS_VENDOR={has_vendor}")
    print("-" * 60)
    for name, ext, target, vendor in rows:
        mark = " " if target else "!"
        print(f"{mark} {name:<22} {vendor:<10} {target}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
