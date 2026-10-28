"""从注册表卸载项导出已安装程序清单，作为独立于开始菜单的留出集。

只读，不写注册表。
"""

import json
import os
import winreg

ROOTS = [
    (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
    (winreg.HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
]

# 系统补丁、运行库、驱动这类不是「用户会放到桌面上的应用」，剔除
SKIP_KEYWORDS = (
    "update for", "hotfix", "security update", "kb2", "redistributable",
    "runtime", "sdk", "framework", "driver", "language pack", "语言包",
    "microsoft visual c++", "microsoft .net", ".net ", "directx",
    "windows software development kit", "windows sdk",
)


def read(root, path):
    out = []
    try:
        key = winreg.OpenKey(root, path)
    except OSError:
        return out
    n = winreg.QueryInfoKey(key)[0]
    for i in range(n):
        try:
            name = winreg.EnumKey(key, i)
        except OSError:
            continue
        try:
            sub = winreg.OpenKey(key, name)
        except OSError:
            continue

        def get(field):
            try:
                return winreg.QueryValueEx(sub, field)[0]
            except OSError:
                return ""

        display = get("DisplayName")
        if not display:
            continue
        if get("SystemComponent") == 1:
            continue
        if get("ParentKeyName"):
            continue
        low = display.lower()
        if any(k in low for k in SKIP_KEYWORDS):
            continue
        out.append({
            "name": display,
            "publisher": get("Publisher"),
            "install_location": get("InstallLocation"),
            "display_icon": get("DisplayIcon"),
        })
    return out


def main():
    seen = set()
    rows = []
    for root, path in ROOTS:
        for r in read(root, path):
            key = r["name"].strip().lower()
            if key in seen:
                continue
            seen.add(key)
            rows.append(r)

    rows.sort(key=lambda r: r["name"].lower())
    os.makedirs("eval", exist_ok=True)
    with open("eval/installed.json", "w", encoding="utf-8") as f:
        json.dump(rows, f, ensure_ascii=False, indent=2)
    print(f"已安装程序 {len(rows)} 条 -> eval/installed.json")


if __name__ == "__main__":
    main()
