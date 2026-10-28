"""M0 完整探针：LNK 结构解析 + PE 版本信息 + 规则分类。

验证 M0 的三项通过条件：
  1. 目标路径覆盖率
  2. 公司名/产品名覆盖率
  3. 难点样本（Zed/Yaak/Sciland/夸克）能否正确归类
"""
import ctypes
import struct
import sys
from ctypes import wintypes
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from parse_lnk import parse_lnk  # noqa: E402

DESKTOP = Path.home() / "Desktop"
VER = ctypes.windll.version


def version_strings(path: str) -> dict:
    try:
        size = VER.GetFileVersionInfoSizeW(path, None)
        if not size:
            return {}
        buf = ctypes.create_string_buffer(size)
        if not VER.GetFileVersionInfoW(path, 0, size, buf):
            return {}
        p, n = ctypes.c_void_p(), wintypes.UINT()
        if not VER.VerQueryValueW(buf, "\\VarFileInfo\\Translation", ctypes.byref(p), ctypes.byref(n)):
            return {}
        if n.value < 4:
            return {}
        lang, cp = struct.unpack("<HH", ctypes.string_at(p.value, 4))
        out = {}
        for key in ("CompanyName", "ProductName", "FileDescription"):
            sub = f"\\StringFileInfo\\{lang:04x}{cp:04x}\\{key}"
            p2, n2 = ctypes.c_void_p(), wintypes.UINT()
            if VER.VerQueryValueW(buf, sub, ctypes.byref(p2), ctypes.byref(n2)) and n2.value:
                out[key] = ctypes.wstring_at(p2.value, n2.value).rstrip("\x00")
        return out
    except Exception:
        return {}


# 内置规则包雏形：按公司 / 路径关键词映射分组
VENDOR_RULES = [
    ("valve", "游戏"), ("epic games", "游戏"), ("steam", "游戏"),
    ("wegame", "游戏"), ("netease\\gameviewer", "游戏"),
    ("blizzard", "游戏"), ("ubisoft", "游戏"),
    ("tencent\\weixin", "社交"), ("tencent\\qqnt", "社交"),
    ("telegram", "社交"), ("bilibili", "影音"), ("cloudmusic", "影音"),
    ("potplayer", "影音"), ("daum", "影音"), ("quark", "影音"),
    ("obs", "创作"), ("obsidian", "创作"),
    ("zed", "开发"), ("yaak", "开发"), ("jetbrains", "开发"),
    ("vmware", "开发"), ("github", "开发"), ("microsoft\\vs", "开发"),
    ("google\\chrome", "浏览器"), ("microsoft\\edge", "浏览器"),
    ("kingsoft", "办公"), ("wps", "办公"), ("feishu", "办公"),
    ("doubao", "办公"), ("bytedance", "办公"),
    ("nvidia", "工具"), ("clash", "工具"), ("todesk", "工具"),
    ("quicklook", "工具"), ("leigod", "工具"), ("sciland", "工具"),
    ("mi\\pccontinuity", "工具"),
]


def classify(name: str, target: str, info: dict) -> str:
    hay = " ".join([
        target.lower(),
        (info.get("CompanyName") or "").lower(),
        (info.get("ProductName") or "").lower(),
        name.lower(),
    ])
    for key, group in VENDOR_RULES:
        if key in hay:
            return group
    return "待归类"


def main() -> int:
    rows = []
    for f in sorted(DESKTOP.iterdir()):
        if not f.is_file():
            continue
        ext = f.suffix.lower()
        target = ""
        if ext == ".lnk":
            r = parse_lnk(f)
            target = r["target"]
            if not target and r["workdir"] and r["relpath"]:
                target = str(Path(r["workdir"]) / r["relpath"])
        elif ext == ".url":
            try:
                for line in f.read_text(errors="ignore").splitlines():
                    if line.upper().startswith("URL="):
                        target = line[4:].strip()
                        break
            except Exception:
                pass
        else:
            continue

        info = version_strings(target) if target and Path(target).exists() else {}
        rows.append({
            "name": f.stem,
            "ext": ext,
            "target": target,
            "company": info.get("CompanyName", ""),
            "product": info.get("ProductName", ""),
            "group": classify(f.stem, target, info),
        })

    total = len(rows)
    has_target = sum(1 for r in rows if r["target"])
    has_company = sum(1 for r in rows if r["company"])
    unclassified = [r["name"] for r in rows if r["group"] == "待归类"]

    print(f"TOTAL={total}  HAS_TARGET={has_target}  HAS_COMPANY={has_company}  "
          f"UNCLASSIFIED={len(unclassified)}")
    print("=" * 78)
    for r in rows:
        print(f"{r['group']:<6} {r['name']:<22} {(r['company'] or '-')[:26]:<28} {r['target'][:60]}")

    print("-" * 78)
    groups = {}
    for r in rows:
        groups.setdefault(r["group"], []).append(r["name"])
    for g, items in sorted(groups.items(), key=lambda x: -len(x[1])):
        print(f"{g}({len(items)}): {'、'.join(items)}")

    if unclassified:
        print(f"\n待归类: {'、'.join(unclassified)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
