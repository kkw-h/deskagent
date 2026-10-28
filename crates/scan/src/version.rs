//! 读取可执行文件的 PE 版本信息（VS_VERSIONINFO 资源）。
//!
//! 这是分类最可靠的信号：`Zed.exe` 能读出 "Zed Industries, Inc."，
//! `Yaak.exe` 能读出 "Yaak"，光看文件名是拿不到的。
//!
//! 用 pelite 纯 Rust 解析，不依赖任何 Windows API——这样整个项目
//! 不需要 MSVC 或 MinGW 的 dlltool，构建链简单很多。

use pelite::FileMap;

#[derive(Debug, Default, Clone)]
pub struct PeVersion {
    pub company: String,
    pub product: String,
    pub description: String,
    pub file_version: String,
}

impl PeVersion {
    pub fn is_empty(&self) -> bool {
        self.company.is_empty() && self.product.is_empty() && self.description.is_empty()
    }
}

/// 读取指定文件的版本信息。文件不存在、不是 PE、或没有版本资源时返回 None。
///
/// 版本资源里常有多个语言块，而第一个往往字段不全（例如 NVIDIA App 的
/// 首个语言块 CompanyName 为空）。所以遍历所有语言，逐字段取第一个非空值，
/// 并优先采用英文块（0x0409）——它的字段最完整。
pub fn read(path: &str) -> Option<PeVersion> {
    let map = FileMap::open(path).ok()?;
    let pe = pelite::PeFile::from_bytes(&map).ok()?;
    let resources = pe.resources().ok()?;
    let version = resources.version_info().ok()?;

    let mut langs: Vec<_> = version.translation().to_vec();
    // 英文块排前面
    langs.sort_by_key(|l| if l.lang_id == 0x0409 { 0 } else { 1 });

    let mut out = PeVersion::default();
    for lang in langs {
        if out.company.is_empty() {
            out.company = version.value(lang, "CompanyName").unwrap_or_default();
        }
        if out.product.is_empty() {
            out.product = version.value(lang, "ProductName").unwrap_or_default();
        }
        if out.description.is_empty() {
            out.description = version.value(lang, "FileDescription").unwrap_or_default();
        }
        if out.file_version.is_empty() {
            out.file_version = version.value(lang, "FileVersion").unwrap_or_default();
        }
        if !out.company.is_empty() && !out.product.is_empty() {
            break;
        }
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}
