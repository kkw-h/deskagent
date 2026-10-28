//! LNK (Shell Link) 二进制解析。
//!
//! 不依赖 COM，按 [MS-SHLLINK] 结构直接读取。覆盖率的差别都在这里：
//! 用正则抓路径只能到 ~79%，正确处理 LinkInfo 才能到 100%。
//!
//! 目标路径的取值优先级：
//!   LinkInfo.LocalBasePathUnicode + CommonPathSuffixUnicode  (最可靠)
//! > LinkInfo.LocalBasePath + CommonPathSuffix               (ANSI / GBK)
//! > RelativePath 拼 WorkingDir                              (兜底)

use std::path::Path;

/// ShellLinkHeader 固定长度。
const HEADER_SIZE: usize = 0x4C;
/// 文件头前 4 字节是 HeaderSize，恒为 0x4C。
const HEADER_MAGIC: u32 = 0x4C;

// LinkFlags，偏移 0x14
const HAS_LINK_TARGET_ID_LIST: u32 = 0x0000_0001;
const HAS_LINK_INFO: u32 = 0x0000_0002;
const HAS_NAME: u32 = 0x0000_0004;
const HAS_RELATIVE_PATH: u32 = 0x0000_0008;
const HAS_WORKING_DIR: u32 = 0x0000_0010;
const HAS_ARGUMENTS: u32 = 0x0000_0020;
const HAS_ICON_LOCATION: u32 = 0x0000_0040;

/// LinkInfo 的 HeaderSize 达到这个值才有 Unicode 路径字段。
const LINK_INFO_UNICODE_MIN_HEADER: u32 = 0x24;

#[derive(Debug, Default, Clone)]
pub struct LnkInfo {
    /// 目标可执行文件或文档的绝对路径。
    pub target: String,
    pub relative_path: String,
    pub working_dir: String,
    pub arguments: String,
    pub icon_location: String,
    /// 快捷方式显示名（LNK 内嵌，可能为空）。
    pub name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LnkError {
    #[error("file too small: {0} bytes")]
    TooSmall(usize),
    #[error("bad header magic (expected 0x4C)")]
    BadMagic,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub fn parse_file(path: &Path) -> Result<LnkInfo, LnkError> {
    let bytes = std::fs::read(path)?;
    parse(&bytes)
}

pub fn parse(b: &[u8]) -> Result<LnkInfo, LnkError> {
    if b.len() < HEADER_SIZE {
        return Err(LnkError::TooSmall(b.len()));
    }
    if read_u32(b, 0) != Some(HEADER_MAGIC) {
        return Err(LnkError::BadMagic);
    }

    let flags = read_u32(b, 0x14).unwrap_or(0);
    let mut off = HEADER_SIZE;
    let mut info = LnkInfo::default();

    // --- LinkTargetIDList ---
    if flags & HAS_LINK_TARGET_ID_LIST != 0 {
        let size = read_u16(b, off).unwrap_or(0) as usize;
        off = off.saturating_add(2).saturating_add(size);
    }

    // --- LinkInfo ---
    if flags & HAS_LINK_INFO != 0 {
        if let Some(li_size) = read_u32(b, off).map(|v| v as usize) {
            if let Some(li) = b.get(off..off.saturating_add(li_size)) {
                if let Some(hdr) = read_u32(li, 4) {
                    let lbp = read_u32(li, 16).unwrap_or(0) as usize;
                    let cps = read_u32(li, 24).unwrap_or(0) as usize;

                    let mut base = read_ansi(li, lbp).unwrap_or_default();
                    let mut suffix = read_ansi(li, cps).unwrap_or_default();

                    if hdr >= LINK_INFO_UNICODE_MIN_HEADER {
                        let lbpu = read_u32(li, 28).unwrap_or(0) as usize;
                        let cpsu = read_u32(li, 32).unwrap_or(0) as usize;
                        if let Some(u) = read_wide(li, lbpu).filter(|s| !s.is_empty()) {
                            base = u;
                        }
                        if let Some(u) = read_wide(li, cpsu).filter(|s| !s.is_empty()) {
                            suffix = u;
                        }
                    }
                    info.target = format!("{base}{suffix}");
                }
            }
            off = off.saturating_add(li_size);
        }
    }

    // --- StringData，顺序固定 ---
    info.name = read_string_data(b, &mut off, flags, HAS_NAME);
    info.relative_path = read_string_data(b, &mut off, flags, HAS_RELATIVE_PATH);
    info.working_dir = read_string_data(b, &mut off, flags, HAS_WORKING_DIR);
    info.arguments = read_string_data(b, &mut off, flags, HAS_ARGUMENTS);
    info.icon_location = read_string_data(b, &mut off, flags, HAS_ICON_LOCATION);

    // 兜底：没有 LinkInfo 时用相对路径拼工作目录
    if info.target.is_empty() && !info.relative_path.is_empty() {
        info.target = if info.working_dir.is_empty() {
            info.relative_path.clone()
        } else {
            format!(
                "{}\\{}",
                info.working_dir.trim_end_matches('\\'),
                info.relative_path.trim_start_matches(".\\")
            )
        };
    }

    Ok(info)
}

fn read_string_data(b: &[u8], off: &mut usize, flags: u32, bit: u32) -> String {
    if flags & bit == 0 {
        return String::new();
    }
    let count = read_u16(b, *off).unwrap_or(0) as usize;
    let start = *off + 2;
    let end = start.saturating_add(count * 2);
    *off = end;
    if end > b.len() {
        return String::new();
    }
    let chars: Vec<u16> = b[start..end]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&chars)
}

fn read_u16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn read_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// 读 NUL 结尾的 UTF-16LE 字符串。
fn read_wide(b: &[u8], off: usize) -> Option<String> {
    if off == 0 || off + 1 >= b.len() {
        return None;
    }
    let mut end = off;
    while end + 1 < b.len() {
        if b[end] == 0 && b[end + 1] == 0 {
            break;
        }
        end += 2;
    }
    let chars: Vec<u16> = b[off..end]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    Some(String::from_utf16_lossy(&chars))
}

/// 读 NUL 结尾的 ANSI 字符串。中文 Windows 上是 GBK。
fn read_ansi(b: &[u8], off: usize) -> Option<String> {
    if off == 0 || off >= b.len() {
        return None;
    }
    let end = b[off..]
        .iter()
        .position(|&c| c == 0)
        .map(|p| off + p)
        .unwrap_or(b.len());
    let (cow, _, _) = encoding_rs::GBK.decode(&b[off..end]);
    Some(cow.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_input() {
        assert!(matches!(parse(&[0u8; 10]), Err(LnkError::TooSmall(10))));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut buf = vec![0u8; 0x4C];
        buf[0] = 0xFF;
        assert!(matches!(parse(&buf), Err(LnkError::BadMagic)));
    }

    #[test]
    fn accepts_minimal_header() {
        let mut buf = vec![0u8; 0x4C];
        buf[0] = 0x4C;
        let info = parse(&buf).expect("minimal header should parse");
        assert!(info.target.is_empty());
    }
}
