"""把注册表导出的已安装程序清单转成 deskagent classify-json 的输入格式。

留出集的意义：这批样本来自注册表，和开始菜单、桌面都不重叠，
也从未参与规则包的编写，用来检验规则包在「没见过的机器」上的覆盖率。
"""

import json
import os
import re

GROUP_EXPECT = {
    # 已知答案的抽查项，用于同时看准确率。其余条目不标注，只看覆盖率。
    "7-Zip 25.01 (x64)": "工具",
    "Bonjour": "工具",
    "Clash Verge": "工具",
    "Deskflow": "工具",
    "Epic Games Launcher": "游戏",
    "Git": "开发",
    "GitHub CLI": "开发",
    "Google Chrome": "浏览器",
    "Logitech G HUB": "工具",
    "Microsoft Edge": "浏览器",
    "Microsoft Visual Studio Installer": "开发",
    "Node.js": "开发",
    "NVIDIA App 11.0.8.244": "工具",
    "OBS Studio": "创作",
    "PotPlayer-64 bit": "影音",
    "QQ": "社交",
    "QuickLook": "工具",
    "Sciland 0.1.243": "工具",
    "Steam": "游戏",
    "Telegram Desktop": "社交",
    "ToDesk": "工具",
    "UU远程": "工具",
    "Visual Studio 生成工具 2022": "开发",
    "VMware Workstation": "开发",
    "WeGame": "游戏",
    "WorkBuddy AI 5.8.4": "开发",
    "WPS Office (12.1.0.28505)": "办公",
    "Yaak": "开发",
    "Yubico Authenticator": "工具",
    "Zed": "开发",
    "双点博物馆": "游戏",
    "双点校园": "游戏",
    "哔哩哔哩": "影音",
    "夸克": "浏览器",
    "小米互联服务": "工具",
    "微信": "社交",
    "微信开发者工具 2.01.2510260": "开发",
    "抖音": "影音",
    "游戏加加轻量版": "工具",
    "网易云音乐": "影音",
    "英雄联盟": "游戏",
    "豆包": "办公",
    "豆包输入法": "工具",
    "雷神加速器": "工具",
    "飞书": "办公",
}


def clean_path(s: str) -> str:
    """去掉注册表里常见的引号和图标索引后缀。"""
    s = (s or "").strip().strip('"').strip()
    s = re.sub(r",\s*-?\d+$", "", s)
    return s.strip().strip('"')


def main():
    rows = json.load(open("eval/installed.json", encoding="utf-8"))
    items = []
    for r in rows:
        target = clean_path(r.get("install_location") or "")
        if not target:
            target = clean_path(r.get("display_icon") or "")
        ext = os.path.splitext(target)[1].lower()
        items.append({
            "name": r["name"],
            "target": target,
            "company": r.get("publisher") or "",
            "ext": ext,
        })

    with open("eval/installed_items.json", "w", encoding="utf-8") as f:
        json.dump(items, f, ensure_ascii=False, indent=2)

    with open("eval/installed_labels.yaml", "w", encoding="utf-8") as f:
        f.write("# 留出集抽查标注：只标了能确定答案的条目，其余仅看覆盖率\n")
        f.write("labels:\n")
        for r in rows:
            g = GROUP_EXPECT.get(r["name"])
            if g:
                f.write(f"  {json.dumps(r['name'], ensure_ascii=False)}: {g}\n")

    print(f"{len(items)} 条 -> eval/installed_items.json，抽查标注 {len(GROUP_EXPECT)} 条")


if __name__ == "__main__":
    main()
