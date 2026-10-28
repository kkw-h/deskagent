//! 桌面采集：枚举条目、解析快捷方式、读取 PE 版本信息。

pub mod lnk;
pub mod version;

pub use lnk::{parse, parse_file, LnkError, LnkInfo};
pub use version::{read as read_version, PeVersion};
